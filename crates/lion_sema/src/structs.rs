//! Structures (spec §12): declarations, construction, fields and invariants.
//!
//! The structures of the file are registered before anything else, so that every type
//! can name them. A structure with conditions gets two functions made by the checker:
//! its *validator*, which gives `none` for a valid value and otherwise says which
//! condition fails, and its *constructor*, which gives the value or an Error (§12.3).
//! A construction from constants is checked at compile time instead, and needs neither
//! (D39).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, StructRef, Type};
use lion_syntax::ast;

use crate::consteval::{self, Const, Env};
use crate::flow::Assigned;
use crate::names::closest;
use crate::{Checker, Context, ContextKind, LocalInfo, article, capitalize, typed};

pub(crate) struct StructInfo<'a> {
    pub(crate) decl: &'a ast::StructDecl,
    /// The file that declares it.
    pub(crate) module: usize,
    pub(crate) id: StructRef,
    pub(crate) fields: Vec<FieldInfo>,
    state: State,
    /// Whether the fields and their default values have no error.
    valid: bool,
    /// The conditions, once checked, in the order of the declaration.
    conditions: Vec<CheckedCondition>,
    /// Whether the conditions could all be checked, so that constants can be.
    conditions_known: bool,
    /// The locals of the validator that hold the fields, by position.
    field_locals: Vec<ir::LocalId>,
    /// The instances of the validator and of the constructor, for a structure with
    /// conditions.
    validator: Option<usize>,
    constructor: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unchecked,
    Checking,
    Checked,
}

#[derive(Clone)]
pub(crate) struct FieldInfo {
    pub(crate) name: String,
    pub(crate) span: Span,
    /// `None` when the written type has an error.
    pub(crate) ty: Option<Type>,
    /// Known once the structure is checked: a constant (C48).
    default: Option<Const>,
    has_default: bool,
    /// `private`: read and changed only in the file of the structure (D10).
    private: bool,
}

struct CheckedCondition {
    expr: ir::Expr,
    span: Span,
    text: String,
    /// The fields that the condition reads, by position.
    fields: Vec<usize>,
}

/// A value given to build a structure: an argument of `Student(...)`, or an element of
/// `(...) as Student` (§12.2).
pub(crate) struct Given<'e> {
    pub(crate) name: Option<&'e ast::Ident>,
    pub(crate) var: Option<Span>,
    pub(crate) value: &'e ast::Expr,
}

impl<'e> Given<'e> {
    pub(crate) fn args(args: &'e [ast::Arg]) -> Vec<Given<'e>> {
        args.iter()
            .map(|arg| Given { name: arg.name.as_ref(), var: arg.var_marker, value: &arg.value })
            .collect()
    }

    pub(crate) fn elements(elements: &'e [ast::Element]) -> Vec<Given<'e>> {
        elements
            .iter()
            .map(|element| Given { name: element.name.as_ref(), var: None, value: &element.value })
            .collect()
    }
}

/// The result of checking the conditions on constants.
enum Verdict {
    Valid,
    /// The condition at this position is false.
    Invalid(usize),
    /// Not known before the run.
    Unknown,
}

impl<'a> Checker<'a> {
    /// Registers the structures of the file, then the types of their fields, which may
    /// name any of them.
    pub(crate) fn register_types(&mut self, module: &'a ast::Module) {
        for stmt in &module.stmts {
            match &stmt.kind {
                ast::StmtKind::Struct(decl) => self.register_structure(decl),
                ast::StmtKind::TypeDef(decl) => self.register_type_definition(decl),
                ast::StmtKind::Trait(decl) => self.register_trait(decl),
                _ => {}
            }
        }
    }

    /// The name of a type of the module being checked, as messages and values show it:
    /// qualified by the module, except in the script.
    pub(crate) fn qualified(&self, name: &str) -> String {
        // A module is reached by the last part of its path (C61).
        match self.module {
            0 => name.to_string(),
            module => {
                let path = &self.modules[module].name;
                format!("{}.{name}", path.rsplit('.').next().unwrap_or(path))
            }
        }
    }

    fn register_structure(&mut self, decl: &'a ast::StructDecl) {
        let name = &decl.name.name;
        if !self.check_type_name(&decl.name) {
            return;
        }
        self.tables.type_spans.insert(name.clone(), decl.name.span);
        self.tables.struct_names.insert(name.clone(), self.structs.len());
        let qualified = self.qualified(name);
        self.structs.push(StructInfo {
            decl,
            id: StructRef::new(&qualified),
            module: self.module,
            fields: Vec::new(),
            state: State::Unchecked,
            valid: true,
            conditions: Vec::new(),
            conditions_known: false,
            field_locals: Vec::new(),
            validator: None,
            constructor: None,
        });
    }

    pub(crate) fn resolve_fields(&mut self, index: usize) {
        let previous = self.enter_module(self.structs[index].module);
        self.resolve_fields_here(index);
        self.enter_module(previous);
    }

    fn resolve_fields_here(&mut self, index: usize) {
        let decl = self.structs[index].decl;
        let mut fields: Vec<FieldInfo> = Vec::new();
        let mut valid = true;
        for line in &decl.lines {
            let ast::StructLine::Field(field) = line else { continue };
            if let Some(previous) = fields.iter().find(|other| other.name == field.name.name) {
                self.diagnostics.push(
                    Diagnostic::error(format!("the field `{}` is declared twice", field.name.name))
                        .with_primary(field.name.span, "")
                        .with_secondary(previous.span, "first declared here"),
                );
                valid = false;
                continue;
            }
            let ty = self.resolve_type(&field.ty);
            valid &= ty.is_some();
            fields.push(FieldInfo {
                name: field.name.name.clone(),
                span: field.name.span,
                ty,
                default: None,
                has_default: field.default.is_some(),
                private: field.private.is_some(),
            });
        }
        let info = &mut self.structs[index];
        info.fields = fields;
        info.valid = valid;
    }

    /// Checks the default values and the conditions of every structure (§12.1).
    pub(crate) fn check_structures(&mut self) {
        for index in 0..self.structs.len() {
            let span = self.structs[index].decl.name.span;
            self.ensure_structure(index, span);
        }
    }

    /// Whether the structure is checked and can be built; `use_span` is where it is
    /// needed, for the message about a structure needed by its own conditions.
    fn ensure_structure(&mut self, index: usize, use_span: Span) -> bool {
        match self.structs[index].state {
            State::Checked => self.structs[index].valid,
            State::Checking => {
                let decl = self.structs[index].decl;
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{}` is needed by its own default values or conditions",
                        decl.name.name
                    ))
                    .with_primary(use_span, "")
                    .with_secondary(decl.name.span, "declared here")
                    .with_help("write the return type of the functions that the conditions call, so that their bodies are checked later"),
                );
                false
            }
            State::Unchecked => {
                self.structs[index].state = State::Checking;
                let previous = self.enter_module(self.structs[index].module);
                let interrupted =
                    std::mem::replace(&mut self.ctx, Context::new(ContextKind::Structure(index)));
                self.check_defaults(index);
                // The validator starts from its own context: its first local is `self`.
                self.ctx = Context::new(ContextKind::Structure(index));
                self.check_conditions(index);
                self.ctx = interrupted;
                self.enter_module(previous);
                self.structs[index].state = State::Checked;
                self.structs[index].valid
            }
        }
    }

    /// The default values, which are constants (C48).
    fn check_defaults(&mut self, index: usize) {
        let decl = self.structs[index].decl;
        for line in &decl.lines {
            let ast::StructLine::Field(field) = line else { continue };
            // A field declared twice is reported already.
            let fields = &self.structs[index].fields;
            let Some(position) = fields.iter().position(|info| info.span == field.name.span) else {
                continue;
            };
            let (Some(default), Some(ty)) = (&field.default, fields[position].ty) else { continue };
            let context = (field.ty.span, "the type of the field".to_string());
            let value =
                self.expr_expecting(default, ty).and_then(|value| self.coerce(value, ty, Some(context)));
            let Some(value) = value else {
                self.structs[index].valid = false;
                continue;
            };
            match consteval::eval(&value, &mut Env::new()) {
                Some(constant) => self.structs[index].fields[position].default = Some(constant),
                None => {
                    self.diagnostics.push(
                        Diagnostic::error("the default value of a field is a constant")
                            .with_primary(default.span, "this is computed while the program runs")
                            .with_note("a default value is written with literals, lists and structures built from constants (C48)"),
                    );
                    self.structs[index].valid = false;
                }
            }
        }
    }

    /// The conditions of the fields and the invariants, checked in the validator, whose
    /// locals are the value and its fields (§12.1).
    fn check_conditions(&mut self, index: usize) {
        let decl = self.structs[index].decl;
        let conditions: Vec<&ast::Condition> = decl
            .lines
            .iter()
            .flat_map(|line| match line {
                ast::StructLine::Field(field) => field.conditions.iter().collect::<Vec<_>>(),
                ast::StructLine::Invariant(condition) => vec![condition],
            })
            .collect();
        // The conditions need the types of the fields, not their default values.
        if conditions.is_empty() || self.structs[index].fields.iter().any(|field| field.ty.is_none()) {
            return;
        }
        let name = decl.name.name.clone();
        let id = self.structs[index].id;
        let ty = Type::Struct(id);
        let problem = Type::maybe(Type::Text);
        let validator = self.synthetic_instance(format!("{name}.check"), decl.name.span, problem);
        let constructor =
            self.synthetic_instance(name.clone(), decl.name.span, Type::union([ty, Type::Error]));
        self.structs[index].validator = Some(validator);
        self.structs[index].constructor = Some(constructor);

        // The validator: `self`, then one constant per field.
        let span = decl.name.span;
        let value = self.parameter("self", ty, false, span);
        let fields = self.structs[index].fields.clone();
        let mut body = Vec::new();
        let mut field_locals = Vec::new();
        for (position, field) in fields.iter().enumerate() {
            let ident = ast::Ident { name: field.name.clone(), span: field.span };
            let field_ty = field.ty.expect("the fields of a valid structure have a type");
            let local = self.declare(&ident, Some(field_ty), false, true);
            let object = Box::new(typed(ir::ExprKind::Local(value), ty, span));
            let read = typed(ir::ExprKind::Field { object, field: position as u32 }, field_ty, span);
            body.push(ir::Stmt::Assign { place: ir::Place::Local(local), value: read });
            field_locals.push(local);
        }
        let mut checked = Vec::new();
        let mut known = true;
        for condition in conditions {
            let Some(expr) = self.expr(&condition.expr) else {
                known = false;
                continue;
            };
            if expr.ty != Type::Bool {
                self.diagnostics.push(
                    Diagnostic::error("a condition of a structure must be a Bool")
                        .with_primary(expr.span, format!("this is {}", article(expr.ty))),
                );
                known = false;
                continue;
            }
            let mut read = Vec::new();
            names_in(&condition.expr, &mut read);
            let used = fields
                .iter()
                .enumerate()
                .filter(|(_, field)| read.contains(&field.name))
                .map(|(position, _)| position)
                .collect();
            checked.push(CheckedCondition {
                expr,
                span: condition.expr.span,
                text: condition.text.clone(),
                fields: used,
            });
        }
        if !known {
            self.structs[index].valid = false;
            return;
        }
        for condition in &checked {
            let message = self.condition_message(condition, &fields, &field_locals);
            let span = condition.span;
            let failed = typed(
                ir::ExprKind::Unary { op: ir::UnaryOp::Not, operand: Box::new(condition.expr.clone()) },
                Type::Bool,
                span,
            );
            body.push(ir::Stmt::If {
                cond: failed,
                then: vec![ir::Stmt::Return(Some(message))],
                otherwise: Vec::new(),
            });
        }
        body.push(ir::Stmt::Return(Some(typed(ir::ExprKind::None, Type::None, span))));
        self.close_scope();
        let ctx = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Structure(index)));
        self.finish_synthetic(validator, vec![ty], ctx.locals, body, ctx.calls);

        // The constructor: the fields as parameters; the value, or an Error that says
        // which condition fails.
        let params: Vec<ir::LocalId> = fields
            .iter()
            .map(|field| {
                self.parameter(&field.name, field.ty.expect("a valid field has a type"), false, field.span)
            })
            .collect();
        let built = self.temporary(ty, span);
        let found = self.temporary(problem, span);
        let values = params
            .iter()
            .zip(&fields)
            .map(|(param, field)| typed(ir::ExprKind::Local(*param), field.ty.expect("typed"), span))
            .collect();
        let read = |local: ir::LocalId, ty: Type| typed(ir::ExprKind::Local(local), ty, span);
        let check = ir::ExprKind::Call {
            function: ir::FunctionId(validator as u32),
            args: vec![ir::Arg::Value(read(built, ty))],
        };
        let message = typed(
            ir::ExprKind::Concat(vec![
                typed(ir::ExprKind::Text(format!("invalid {name}: ")), Type::Text, span),
                read(found, Type::Text),
            ]),
            Type::Text,
            span,
        );
        let error = typed(
            ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Error, args: vec![message] },
            Type::Error,
            span,
        );
        let body = vec![
            ir::Stmt::Assign {
                place: ir::Place::Local(built),
                value: typed(ir::ExprKind::Struct { structure: id, fields: values }, ty, span),
            },
            ir::Stmt::Assign { place: ir::Place::Local(found), value: typed(check, problem, span) },
            ir::Stmt::If {
                cond: typed(
                    ir::ExprKind::TypeTest { value: Box::new(read(found, problem)), ty: Type::Text },
                    Type::Bool,
                    span,
                ),
                then: vec![ir::Stmt::Return(Some(error))],
                otherwise: Vec::new(),
            },
            ir::Stmt::Return(Some(read(built, ty))),
        ];
        let param_types = fields.iter().map(|field| field.ty.expect("typed")).collect();
        let ctx = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Structure(index)));
        self.finish_synthetic(constructor, param_types, ctx.locals, body, vec![validator]);

        let info = &mut self.structs[index];
        info.conditions = checked;
        info.conditions_known = true;
        info.field_locals = field_locals;
    }

    /// A parameter of a function made by the checker.
    fn parameter(&mut self, name: &str, ty: Type, mutable: bool, span: Span) -> ir::LocalId {
        let id = self.push_local(LocalInfo {
            name: name.to_string(),
            ty: Some(ty),
            mutable,
            temporary: false,
            by_reference: false,
            decl_span: span,
            initialized: true,
            first_assignment: None,
            loop_variable: false,
            captured: false,
            boxed: false,
        });
        self.ctx.flow.set(id, Assigned::Yes);
        id
    }

    /// "grade = 25.0 does not satisfy 0 <= grade <= 20", from the fields at run time.
    fn condition_message(
        &self,
        condition: &CheckedCondition,
        fields: &[FieldInfo],
        locals: &[ir::LocalId],
    ) -> ir::Expr {
        let span = condition.span;
        let text = |text: String| typed(ir::ExprKind::Text(text), Type::Text, span);
        let mut parts = Vec::new();
        for (count, &position) in condition.fields.iter().enumerate() {
            let separator = if count == 0 { "" } else { ", " };
            parts.push(text(format!("{separator}{} = ", fields[position].name)));
            let value =
                typed(ir::ExprKind::Local(locals[position]), fields[position].ty.expect("typed"), span);
            let kind = ir::ExprKind::Convert { conversion: ir::Conversion::Literal, value: Box::new(value) };
            parts.push(typed(kind, Type::Text, span));
        }
        if parts.is_empty() {
            parts.push(text(format!("{} is false", condition.text)));
        } else {
            parts.push(text(format!(" does not satisfy {}", condition.text)));
        }
        typed(ir::ExprKind::Concat(parts), Type::Text, span)
    }

    /// The values of the fields, in order, for the values given to build a structure
    /// (§12.2, D55, D70).
    fn field_values(&mut self, index: usize, given: &[Given], span: Span) -> Option<Vec<ir::Expr>> {
        if !self.ensure_structure(index, span) {
            for value in given {
                self.expr(value.value);
            }
            return None;
        }
        let decl = self.structs[index].decl;
        let name = &decl.name.name;
        let fields = self.structs[index].fields.clone();
        let required = fields.iter().rposition(|field| !field.has_default).map_or(0, |position| position + 1);
        if given.len() > fields.len() || given.len() < required {
            let count = if required == fields.len() {
                format!("{required}")
            } else {
                format!("{required} to {}", fields.len())
            };
            let plural = if fields.len() == 1 { "" } else { "s" };
            let list: Vec<String> = fields
                .iter()
                .map(|field| {
                    format!("{} in {}", field.name, field.ty.map_or("?".to_string(), |ty| ty.to_string()))
                })
                .collect();
            let mut error = Diagnostic::error(format!(
                "`{name}` is built from {count} value{plural}, not {}",
                given.len()
            ))
            .with_primary(span, "")
            .with_secondary(decl.name.span, "declared here")
            .with_note(format!("the fields of `{name}`: {}", list.join(", ")));
            if required < fields.len() {
                error =
                    error.with_note("the last fields have a default value and can be left out (§12.2, D70)");
            }
            self.diagnostics.push(error);
            for value in given {
                self.expr(value.value);
            }
            return None;
        }
        let mut values = Vec::new();
        let mut valid = true;
        for (value, field) in given.iter().zip(&fields) {
            if let Some(var) = value.var {
                self.diagnostics.push(
                    Diagnostic::error("a structure is built from values")
                        .with_primary(var, "remove this `var`")
                        .with_note("`var` at a call marks an argument that the function modifies (§11.2)"),
                );
                valid = false;
            }
            if let Some(given_name) = value.name
                && given_name.name != field.name
            {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "this value is named `{}`, but the field at this position is `{}`",
                        given_name.name, field.name
                    ))
                    .with_primary(given_name.span, "")
                    .with_secondary(field.span, "field declared here")
                    .with_note("named values keep the order of the fields (§12.2)"),
                );
                // The value is probably meant for another field: its type is not checked.
                self.expr(value.value);
                valid = false;
                continue;
            }
            let ty = field.ty.expect("the fields of a valid structure have a type");
            let context = (field.span, format!("the field `{}` is {}", field.name, article(ty)));
            match self
                .expr_expecting(value.value, ty)
                .and_then(|checked| self.coerce(checked, ty, Some(context)))
            {
                Some(checked) => values.push(checked),
                None => valid = false,
            }
        }
        if !valid {
            return None;
        }
        for field in &fields[given.len()..] {
            let default = field.default.clone().expect("a valid structure knows its default values");
            values.push(self.const_expr(&default, field.ty.expect("typed"), span));
        }
        Some(values)
    }

    /// `Student(...)`, `(...) as Student`: the value, or a `Student or Error` when the
    /// conditions are checked at run time (§12.2, §12.3, D39).
    pub(crate) fn construct(&mut self, index: usize, given: &[Given], span: Span) -> Option<ir::Expr> {
        let values = self.field_values(index, given, span)?;
        let id = self.structs[index].id;
        let ty = Type::Struct(id);
        let built = |fields| typed(ir::ExprKind::Struct { structure: id, fields }, ty, span);
        let Some(constructor) = self.structs[index].constructor else { return Some(built(values)) };
        match self.verdict(index, &values) {
            Verdict::Valid => return Some(built(values)),
            Verdict::Invalid(position) => {
                let decl = self.structs[index].decl;
                let condition = &self.structs[index].conditions[position];
                let detail = self.failure_detail(index, position, &values);
                self.diagnostics.push(
                    Diagnostic::error(format!("this `{}` is invalid", decl.name.name))
                        .with_primary(span, detail)
                        .with_secondary(condition.span, "condition declared here")
                        .with_note(format!(
                            "the values are constants, so the conditions of `{}` are checked now (§12.3, D39)",
                            decl.name.name
                        )),
                );
                return None;
            }
            Verdict::Unknown => {}
        }
        self.calls_synthetic(constructor, span);
        let args = values.into_iter().map(ir::Arg::Value).collect();
        let call = ir::ExprKind::Call { function: ir::FunctionId(constructor as u32), args };
        Some(typed(call, Type::union([ty, Type::Error]), span))
    }

    /// `(...) in Student`, in parentheses: whether these values build a valid `Student`,
    /// without an Error or a bug (§12.3).
    pub(crate) fn construction_test(
        &mut self,
        index: usize,
        given: &[Given],
        span: Span,
    ) -> Option<ir::Expr> {
        let values = self.field_values(index, given, span)?;
        let id = self.structs[index].id;
        let ty = Type::Struct(id);
        let valid = |value: bool| typed(ir::ExprKind::Bool(value), Type::Bool, span);
        let Some(validator) = self.structs[index].validator else {
            // The values are still evaluated, in order.
            let stmts = values.into_iter().map(ir::Stmt::Expr).collect();
            return Some(typed(
                ir::ExprKind::Block { stmts, value: Box::new(valid(true)) },
                Type::Bool,
                span,
            ));
        };
        match self.verdict(index, &values) {
            Verdict::Valid => return Some(valid(true)),
            Verdict::Invalid(_) => return Some(valid(false)),
            Verdict::Unknown => {}
        }
        self.calls_synthetic(validator, span);
        let value = typed(ir::ExprKind::Struct { structure: id, fields: values }, ty, span);
        let problem = Type::maybe(Type::Text);
        let call = ir::ExprKind::Call {
            function: ir::FunctionId(validator as u32),
            args: vec![ir::Arg::Value(value)],
        };
        let test = ir::ExprKind::TypeTest { value: Box::new(typed(call, problem, span)), ty: Type::None };
        Some(typed(test, Type::Bool, span))
    }

    /// Whether the conditions hold for these values, when they are constants (D39).
    fn verdict(&self, index: usize, values: &[ir::Expr]) -> Verdict {
        let info = &self.structs[index];
        if !info.conditions_known {
            return Verdict::Unknown;
        }
        let mut env = Env::new();
        let Some(constants) =
            values.iter().map(|value| consteval::eval(value, &mut env)).collect::<Option<Vec<_>>>()
        else {
            return Verdict::Unknown;
        };
        let mut env: Env = info.field_locals.iter().copied().zip(constants).collect();
        for (position, condition) in info.conditions.iter().enumerate() {
            match consteval::eval(&condition.expr, &mut env) {
                Some(Const::Bool(true)) => {}
                Some(Const::Bool(false)) => return Verdict::Invalid(position),
                _ => return Verdict::Unknown,
            }
        }
        Verdict::Valid
    }

    /// "grade = 25.0 does not satisfy 0 <= grade <= 20", for constant values.
    fn failure_detail(&self, index: usize, position: usize, values: &[ir::Expr]) -> String {
        let info = &self.structs[index];
        let condition = &info.conditions[position];
        let names = |id: StructRef| self.field_names(id);
        let parts: Vec<String> = condition
            .fields
            .iter()
            .map(|&field| {
                let value =
                    consteval::eval(&values[field], &mut Env::new()).map(|value| value.literal(&names));
                format!("{} = {}", info.fields[field].name, value.unwrap_or_default())
            })
            .collect();
        if parts.is_empty() {
            format!("{} is false", condition.text)
        } else {
            format!("{} does not satisfy {}", parts.join(", "), condition.text)
        }
    }

    /// Statements that stop the program with a bug when `value`, a value of the
    /// structure, does not satisfy its conditions (§12.3, D40).
    pub(crate) fn validation(&mut self, structure: StructRef, value: ir::Expr) -> Vec<ir::Stmt> {
        let index = self.struct_index(structure);
        let span = value.span;
        if !self.ensure_structure(index, span) {
            return Vec::new();
        }
        let Some(validator) = self.structs[index].validator else { return Vec::new() };
        self.calls_synthetic(validator, span);
        let problem = Type::maybe(Type::Text);
        let found = self.temporary(problem, span);
        let call = ir::ExprKind::Call {
            function: ir::FunctionId(validator as u32),
            args: vec![ir::Arg::Value(value)],
        };
        let read = |ty: Type| typed(ir::ExprKind::Local(found), ty, span);
        let name = typed(ir::ExprKind::Text(structure.name()), Type::Text, span);
        let broken =
            ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Broken, args: vec![name, read(Type::Text)] };
        let test = ir::ExprKind::TypeTest { value: Box::new(read(problem)), ty: Type::Text };
        vec![
            ir::Stmt::Assign { place: ir::Place::Local(found), value: typed(call, problem, span) },
            ir::Stmt::If {
                cond: typed(test, Type::Bool, span),
                then: vec![ir::Stmt::Expr(typed(broken, Type::None, span))],
                otherwise: Vec::new(),
            },
        ]
    }

    /// The position and the type of the field `name` of a value of type `ty` (§12.1).
    pub(crate) fn field_of(&mut self, ty: Type, name: &ast::Ident, object: Span) -> Option<(u32, Type)> {
        let Type::Struct(structure) = ty else {
            let mut error =
                Diagnostic::error(format!("{} has no field `{}`", capitalize(&article(ty)), name.name))
                    .with_primary(name.span, "")
                    .with_secondary(object, format!("this is {}", article(ty)));
            if ty.members().iter().any(|member| matches!(member, Type::Struct(_)))
                && let Some(help) = crate::expr::union_help(ty)
            {
                error = error.with_help(help);
            }
            self.diagnostics.push(error);
            return None;
        };
        let index = self.struct_index(structure);
        let fields = &self.structs[index].fields;
        if let Some(position) = fields.iter().position(|field| field.name == name.name) {
            if fields[position].private && self.structs[index].module != self.module {
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "the field `{}` of `{}` is private",
                        name.name,
                        structure.name()
                    ))
                    .with_primary(name.span, "")
                    .with_note("`private` limits a field to the file of its structure (§20.3, D10)"),
                );
                return None;
            }
            return Some((position as u32, fields[position].ty?));
        }
        let mut error = Diagnostic::error(format!("`{}` has no field `{}`", structure.name(), name.name))
            .with_primary(name.span, "")
            .with_secondary(self.structs[index].decl.name.span, "declared here");
        if self.visible_method(ty, &name.name).is_some() {
            error = error.with_note(format!("`{0}` is a method: call it with `.{0}()`", name.name));
        } else if let Some(close) = closest(&name.name, fields.iter().map(|field| field.name.as_str())) {
            error = error.with_help(format!("a similar field exists: `{close}`"));
        }
        self.diagnostics.push(error);
        None
    }

    pub(crate) fn struct_index(&self, structure: StructRef) -> usize {
        self.structs.iter().position(|info| info.id == structure).expect("every structure is registered")
    }

    pub(crate) fn field_names(&self, structure: StructRef) -> Vec<String> {
        self.structs[self.struct_index(structure)].fields.iter().map(|field| field.name.clone()).collect()
    }

    /// The expression that gives a constant, where a value of type `ty` is expected.
    fn const_expr(&self, value: &Const, ty: Type, span: Span) -> ir::Expr {
        let (kind, ty) = match value {
            Const::Int(value) => (ir::ExprKind::Int(*value), Type::Int),
            Const::Float(value) => (ir::ExprKind::Float(*value), Type::Float),
            Const::Bool(value) => (ir::ExprKind::Bool(*value), Type::Bool),
            Const::Text(text) => (ir::ExprKind::Text(text.clone()), Type::Text),
            Const::None => (ir::ExprKind::None, Type::None),
            Const::List(elements) => {
                let list = ty
                    .members()
                    .into_iter()
                    .find(|member| matches!(member, Type::List(_)))
                    .expect("a list goes where a list is expected");
                let element = list.element().expect("a list has elements");
                let elements = elements.iter().map(|value| self.const_expr(value, element, span)).collect();
                (ir::ExprKind::List(elements), list)
            }
            Const::Enum(enumeration, value) => {
                (ir::ExprKind::Enum { enumeration: *enumeration, value: *value }, Type::Enum(*enumeration))
            }
            Const::Set(elements) => {
                let set = ty
                    .members()
                    .into_iter()
                    .find(|member| matches!(member, Type::Set(_)))
                    .expect("a set goes where a set is expected");
                let element = set.element().expect("a set has elements");
                let elements = elements.iter().map(|value| self.const_expr(value, element, span)).collect();
                (ir::ExprKind::Set(elements), set)
            }
            Const::Tuple(elements) => {
                let tuple = ty
                    .members()
                    .into_iter()
                    .find(|member| matches!(member, Type::Tuple(_)))
                    .expect("a tuple goes where a tuple is expected");
                let Type::Tuple(types) = tuple else { unreachable!("a tuple type") };
                let elements = elements
                    .iter()
                    .zip(types.elements())
                    .map(|(value, ty)| self.const_expr(value, ty, span))
                    .collect();
                (ir::ExprKind::Tuple(elements), tuple)
            }
            Const::Struct(structure, values) => {
                let fields = &self.structs[self.struct_index(*structure)].fields;
                let values = values
                    .iter()
                    .zip(fields)
                    .map(|(value, field)| self.const_expr(value, field.ty.expect("typed"), span))
                    .collect();
                (ir::ExprKind::Struct { structure: *structure, fields: values }, Type::Struct(*structure))
            }
        };
        typed(kind, ty, span)
    }
}

/// The lowercase names that an expression reads.
fn names_in(expr: &ast::Expr, names: &mut Vec<String>) {
    use ast::ExprKind::*;
    match &expr.kind {
        Name(name) => names.push(name.clone()),
        Int(_) | Float(_) | Bool(_) | None | TypeName(_) => {}
        Text(parts) => {
            for part in parts {
                if let ast::TextPart::Interpolation(value) = part {
                    names_in(value, names);
                }
            }
        }
        Paren(inner) | Try(inner) | Parallel(inner) => names_in(inner, names),
        Fun(_) => {}
        List(elements) | Set(elements) => elements.iter().for_each(|element| names_in(element, names)),
        Tuple(elements) => elements.iter().for_each(|element| names_in(&element.value, names)),
        Unary { operand, .. } => names_in(operand, names),
        Binary { lhs, rhs, .. } => {
            names_in(lhs, names);
            names_in(rhs, names);
        }
        Compare { first, rest } => {
            names_in(first, names);
            rest.iter().for_each(|comparison| names_in(&comparison.rhs, names));
        }
        As { value, .. } | TypeTest { value, .. } => names_in(value, names),
        Call { callee, args } => {
            names_in(callee, names);
            args.iter().for_each(|arg| names_in(&arg.value, names));
        }
        Field { object, .. } => names_in(object, names),
        Index { object, index } => {
            names_in(object, names);
            names_in(index, names);
        }
        Match { scrutinee, cases } => {
            names_in(scrutinee, names);
            for (case, value) in cases {
                case.conditions.iter().for_each(|condition| names_in(condition, names));
                names_in(value, names);
            }
        }
        If { branches, otherwise } => {
            for (cond, value) in branches {
                names_in(cond, names);
                names_in(value, names);
            }
            if let Some(otherwise) = otherwise {
                names_in(otherwise, names);
            }
        }
    }
}
