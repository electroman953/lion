//! Functions (spec §11): declarations, bodies, calls, and the globals they reach.
//!
//! Top-level functions are registered before the script is checked, so they can be
//! called before their declaration (C4). A function whose parameters all have a type
//! has one *instance*. A parameter without a type makes the function generic (C1): each
//! call creates, or reuses, the instance for the types of its arguments, checked and
//! compiled on its own (§15.3).
//!
//! Each instance records the globals it reads and the instances it calls; once every
//! body is known, the reads are closed over the calls, and each call made by the script
//! is checked: it must not read a global that may have no value at that point (§6.1, C3).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::closures::ClosureInfo;
use crate::flow::Assigned;
use crate::names::Resolved;
use crate::places::Path;
use crate::{Checker, Context, ContextKind, GlobalInfo, GlobalType, LocalInfo, article, ir_locals, typed};

pub(crate) struct FunctionInfo {
    /// Shared, so that a function declared inside a body can be registered too.
    pub(crate) decl: Rc<ast::FunDecl>,
    /// The file that declares it, and how names of that file are qualified: `geometry.`,
    /// or nothing in the script.
    pub(crate) module: usize,
    prefix: String,
    /// For a method, the type of `self` (§12.4).
    pub(crate) receiver: Option<Type>,
    /// A method of `List`, `Set` or `Map`: its first type variables are the types of the
    /// elements (C97). `receiver` is known once the signature is.
    pub(crate) collection: Option<&'static str>,
    /// A method of a structure or an enumeration, declared in the file of its type: it
    /// goes wherever the values of the type go (C87).
    pub(crate) own_method: bool,
    /// For a method of an instance of a generic structure: the types of the parameters of
    /// the structure, known in its signature and its body (§15.1, C90).
    pub(crate) struct_args: Vec<(String, Type)>,
    /// A method whose declaration does not write `self`: its first parameter is added.
    implicit_self: bool,
    /// A method declared with `var self`, which changes its object (§12.4).
    pub(crate) var_self: bool,
    /// A function of the standard library that the implementation provides (§23).
    native: Option<ir::Native>,
    /// A C function: its position in `Checker::foreign`, and whether it is `pure`
    /// (§21.2, D48).
    pub(crate) foreign: Option<(u32, bool)>,
    /// A function declared inside a body: the variables it captures (§11.5).
    pub(crate) closure: Option<ClosureInfo>,
    /// For an anonymous function, the function type expected where it is written.
    expected: Option<ir::FunData>,
    /// The type variables, with the trait each must satisfy (`None` for `Type`) (§15.2).
    pub(crate) type_params: Vec<(ir::VarRef, Option<Type>, Span)>,
    /// A `test` block, by name (§24.1).
    pub(crate) test: Option<String>,
    /// `None` when the declaration has an error or needs what is not implemented.
    pub(crate) signature: Option<Vec<ParamInfo>>,
    /// The return type, when written.
    pub(crate) declared_ret: Option<Type>,
    /// The globals named after `modifies` (§11.5).
    modifies: Vec<ir::LocalId>,
    /// The instances of a generic function, by the types of the given arguments.
    instances: HashMap<Vec<Type>, usize>,
    /// The only instance of a function whose parameters all have a type.
    pub(crate) instance: Option<usize>,
}

impl FunctionInfo {
    fn is_generic(&self) -> bool {
        self.signature
            .as_ref()
            .is_some_and(|params| params.iter().any(|param| param.ty.is_none_or(Type::has_vars)))
    }

    /// `passes`, `Student.passes` for a method, `geometry.area` in a module.
    fn full_name(&self) -> String {
        if let Some(kind) = self.collection {
            return format!("{kind}.{}", self.decl.name.name);
        }
        match self.receiver {
            Some(receiver) => format!("{receiver}.{}", self.decl.name.name),
            None => format!("{}{}", self.prefix, self.decl.name.name),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ParamInfo {
    pub(crate) name: String,
    /// `None` for a parameter without a type, which makes the function generic.
    pub(crate) ty: Option<Type>,
    pub(crate) by_reference: bool,
    pub(crate) has_default: bool,
    pub(crate) span: Span,
}

/// One checked version of a function.
pub(crate) struct Instance {
    /// The function; unused for a function made by the checker.
    function: usize,
    /// For a function made by the checker (§12.3): its name and where it comes from.
    synthetic: Option<(String, Span)>,
    /// For a generic function: the types of the arguments given by the calls of this
    /// instance, which gave that many arguments. Empty otherwise.
    arg_types: Vec<Type>,
    /// The call that created a generic instance, for messages.
    origin: Option<Span>,
    ret: Ret,
    state: BodyState,
    /// The types of the type variables of the function, for this instance (§15.2).
    bindings: HashMap<ir::VarRef, Type>,
    /// Known once the body is checked.
    reads: Vec<ir::LocalId>,
    calls: Vec<usize>,
    checked: Option<CheckedBody>,
}

#[derive(Clone, Copy)]
pub(crate) enum Ret {
    Declared(Type),
    /// Not written: inferred from the body (C12).
    Unknown,
    /// The body is being checked to find it.
    Inferring,
    Inferred(Type),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyState {
    Unchecked,
    Checking,
    Checked,
}

struct CheckedBody {
    params: Vec<Type>,
    /// The number of variables captured by a function declared in a body (§11.5).
    captures: u32,
    locals: Vec<LocalInfo>,
    body: Vec<ir::Stmt>,
    defaults: Vec<(u32, ir::Expr)>,
}

/// A call made by the script.
pub(crate) struct ScriptCall {
    instance: usize,
    span: Span,
    /// The globals that may have no value at the call.
    unassigned: Vec<ir::LocalId>,
}

impl ScriptCall {
    pub(crate) fn new(instance: usize, span: Span, unassigned: Vec<ir::LocalId>) -> ScriptCall {
        ScriptCall { instance, span, unassigned }
    }
}

/// An argument once checked, before the call is built.
pub(crate) enum Pending {
    Value(ir::Expr),
    /// A variable given to a `var` parameter, and its type.
    Place(ir::Place, Type),
    /// A part of a `var` variable given to a `var` parameter: it is copied into a
    /// temporary, given to the function, then stored back (C50). `setup` evaluates the
    /// indices of the path first.
    Element(Path, Vec<ir::Stmt>),
    /// Any other value given to a `var` parameter: the modification is lost (§11.2).
    Temporary(ir::Expr),
}

impl Pending {
    fn ty(&self) -> Type {
        match self {
            Pending::Value(value) | Pending::Temporary(value) => value.ty,
            Pending::Place(_, ty) => *ty,
            Pending::Element(path, _) => path.ty(),
        }
    }
}

impl Instance {
    pub(crate) fn into_ir(self, functions: &[FunctionInfo]) -> ir::Function {
        let checked = self.checked.expect("every instance of a valid program is checked");
        let ret = match self.ret {
            Ret::Declared(ty) | Ret::Inferred(ty) => ty,
            _ => unreachable!("a valid program knows every return type"),
        };
        if let Some((name, span)) = self.synthetic {
            return ir::Function {
                name,
                params: checked.params.len() as u32,
                captures: 0,
                defaults: Vec::new(),
                ret,
                locals: ir_locals(checked.locals),
                body: checked.body,
                span: Some(span),
            };
        }
        let function = &functions[self.function];
        let decl = function.decl.clone();
        let name = if self.arg_types.is_empty() {
            function.full_name()
        } else {
            let types: Vec<String> = checked.params.iter().map(Type::to_string).collect();
            format!("{}[{}]", function.full_name(), types.join(", "))
        };
        ir::Function {
            name,
            params: checked.params.len() as u32,
            captures: checked.captures,
            defaults: checked.defaults,
            ret,
            locals: ir_locals(checked.locals),
            body: checked.body,
            span: Some(decl.name.span),
        }
    }
}

impl<'a> Checker<'a> {
    /// Registers the functions and globals of the file before anything is checked.
    /// Registers the declarations of every file before anything is checked (C4): the
    /// types, then the functions and the globals, then the signatures; then come the
    /// values of the globals of the modules, and the structures.
    pub(crate) fn register_program(&mut self) {
        let count = self.modules.len();
        for module in 0..count {
            self.resolve_imports(module);
            if module > 0 {
                self.check_module_statements(module);
            }
        }
        for module in 0..count {
            self.enter_module(module);
            self.register_types(self.modules[module].ast);
        }
        for module in 0..count {
            self.enter_module(module);
            self.resolve_type_definitions();
        }
        for index in 0..self.structs.len() {
            self.resolve_fields(index);
        }
        self.resolve_traits();
        for module in 0..count {
            self.enter_module(module);
            self.register_top_level(module);
        }
        self.enter_module(0);
        for module in 1..count {
            let has_globals =
                self.modules[module].ast.stmts.iter().any(|stmt| matches!(stmt.kind, ast::StmtKind::Let(_)));
            if has_globals {
                let name = format!("{}.init", self.modules[module].name);
                let span = self.modules[module].ast.stmts[0].span;
                self.modules[module].init = Some(self.synthetic_instance(name, span, Type::None));
            }
        }
        self.signatures_started = true;
        for index in 0..self.functions.len() {
            let previous = self.enter_module(self.functions[index].module);
            self.resolve_signature(index);
            self.enter_module(previous);
            let function = &self.functions[index];
            if function.signature.is_some() && !function.is_generic() {
                let ret = function.declared_ret.map_or(Ret::Unknown, Ret::Declared);
                let instance = self.new_instance(index, Vec::new(), None, ret);
                self.functions[index].instance = Some(instance);
            }
        }
        self.enter_module(0);
        self.make_seeds();
        self.register_equalities();
        self.conform_traits();
        self.check_pending_constraints();
        self.register_tests();
        for module in (1..count).rev() {
            self.check_module_init(module);
        }
        self.check_structures();
        self.enter_module(0);
    }

    /// The functions and the globals of the module being checked.
    fn register_top_level(&mut self, module: usize) {
        let ast = self.modules[module].ast;
        let mut declarations: Vec<&'a ast::LetStmt> = Vec::new();
        for stmt in &ast.stmts {
            match &stmt.kind {
                ast::StmtKind::Fun(decl) => self.register_function(decl),
                // `let twice = fun(x) = x * 2` is a generic function (§11.1, C78).
                ast::StmtKind::Let(decl)
                    if !decl.mutable && crate::closures::generic_function_value(decl).is_some() =>
                {
                    let function = crate::closures::generic_function_value(decl).expect("checked");
                    self.register_function(&ast::FunDecl { name: decl.name.clone(), ..function.clone() });
                }
                ast::StmtKind::Let(decl) => declarations.push(decl),
                _ => {}
            }
        }
        for decl in &declarations {
            let count = declarations.iter().filter(|other| other.name.name == decl.name.name).count();
            if self.tables.globals.contains_key(&decl.name.name) {
                continue;
            }
            // A name declared once gets its local now, so that functions can refer to it.
            let local = (count == 1).then(|| {
                let mut info = LocalInfo::variable(&decl.name, None, decl.mutable, decl.value.is_some());
                if module > 0 {
                    info.name = format!("{}.{}", self.modules[module].name, decl.name.name);
                }
                let local = self.push_local(info);
                self.global_names.insert(local, decl.name.name.clone());
                if module > 0 {
                    self.global_modules.insert(local, module);
                }
                local
            });
            self.tables
                .globals
                .insert(decl.name.name.clone(), GlobalInfo { decl, local, ty: GlobalType::Unknown });
            if let Some(&function) = self.tables.function_names.get(&decl.name.name) {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{}` names both a function and a variable", decl.name.name))
                        .with_primary(decl.name.span, "variable declared here")
                        .with_secondary(self.functions[function].decl.name.span, "function declared here")
                        .with_help("choose different names"),
                );
            }
        }
    }

    /// A function declared inside a body, or anonymous (§11.5).
    pub(crate) fn register_nested_function(
        &mut self,
        decl: Rc<ast::FunDecl>,
        captures: Vec<crate::closures::Capture>,
        expected: Option<ir::FunData>,
    ) {
        self.functions.push(FunctionInfo {
            decl,
            module: self.module,
            prefix: self.qualified(""),
            receiver: None,
            own_method: false,
            collection: None,
            struct_args: Vec::new(),
            implicit_self: false,
            var_self: false,
            native: None,
            foreign: None,
            closure: Some(ClosureInfo { captures }),
            expected,
            type_params: Vec::new(),
            test: None,
            signature: None,
            declared_ret: None,
            modifies: Vec::new(),
            instances: HashMap::new(),
            instance: None,
        });
    }

    /// A default method of a trait, given to a type that satisfies it (§14.2).
    pub(crate) fn register_default_method(
        &mut self,
        decl: Rc<ast::FunDecl>,
        receiver: Type,
        module: usize,
        type_args: Vec<(String, Type)>,
    ) -> usize {
        let index = self.functions.len();
        let name = decl.name.name.clone();
        let previous = self.enter_module(module);
        self.functions.push(FunctionInfo {
            decl,
            module,
            prefix: String::new(),
            receiver: Some(receiver),
            own_method: false,
            collection: None,
            struct_args: type_args,
            implicit_self: false,
            var_self: false,
            native: None,
            foreign: None,
            closure: None,
            expected: None,
            type_params: Vec::new(),
            test: None,
            signature: None,
            declared_ret: None,
            modifies: Vec::new(),
            instances: HashMap::new(),
            instance: None,
        });
        self.methods.entry((receiver, name)).or_default().push(index);
        self.resolve_signature(index);
        let function = &self.functions[index];
        if function.signature.is_some() && !function.is_generic() {
            let ret = function.declared_ret.map_or(Ret::Unknown, Ret::Declared);
            let instance = self.new_instance(index, Vec::new(), None, ret);
            self.functions[index].instance = Some(instance);
        }
        self.enter_module(previous);
        index
    }

    /// The function of an instance, unless the checker made it.
    pub(crate) fn instance_function(&self, instance: usize) -> Option<usize> {
        let instance = &self.instances[instance];
        instance.synthetic.is_none().then_some(instance.function)
    }

    /// The types of the parameters of an instance.
    pub(crate) fn instance_param_types(&self, instance: usize, params: &[ParamInfo]) -> Vec<Type> {
        let arg_types = &self.instances[instance].arg_types;
        params
            .iter()
            .enumerate()
            .map(|(position, param)| param.ty.or(arg_types.get(position).copied()).unwrap_or(Type::None))
            .collect()
    }

    fn register_function(&mut self, decl: &ast::FunDecl) {
        let info = FunctionInfo {
            decl: Rc::new(decl.clone()),
            module: self.module,
            prefix: self.qualified(""),
            receiver: None,
            own_method: false,
            collection: None,
            struct_args: Vec::new(),
            implicit_self: false,
            var_self: false,
            native: None,
            foreign: None,
            closure: None,
            expected: None,
            type_params: Vec::new(),
            test: None,
            signature: None,
            declared_ret: None,
            modifies: Vec::new(),
            instances: HashMap::new(),
            instance: None,
        };
        if let Some(receiver) = &decl.receiver {
            if let Some(&template) = self.tables.generic_structs.get(&receiver.name) {
                self.register_template_method(template, info.decl);
                return;
            }
            // `fun List.second() in maybe T, T in Type` (§12.4, C97).
            if let Some(kind) = ["List", "Set", "Map"].into_iter().find(|kind| *kind == receiver.name) {
                if !self.check_collection_method_name(kind, decl) {
                    return;
                }
                let key = (kind, decl.name.name.clone());
                self.collection_methods.entry(key).or_default().push(self.functions.len());
                self.functions.push(FunctionInfo { collection: Some(kind), ..info });
                return;
            }
            let Some(ty) = self.receiver_type(receiver) else { return };
            if !self.check_method_name(ty, decl) {
                return;
            }
            self.methods.entry((ty, decl.name.name.clone())).or_default().push(self.functions.len());
            let own_method = matches!(ty, Type::Struct(_) | Type::Enum(_));
            self.functions.push(FunctionInfo { receiver: Some(ty), own_method, ..info });
            return;
        }
        if let Some(&previous) = self.tables.function_names.get(&decl.name.name) {
            self.diagnostics.push(
                Diagnostic::error(format!("the function `{}` is already declared", decl.name.name))
                    .with_primary(decl.name.span, "declared again here")
                    .with_secondary(self.functions[previous].decl.name.span, "first declared here")
                    .with_note("each function has its own name: Lion has no overloading"),
            );
            return;
        }
        self.tables.function_names.insert(decl.name.name.clone(), self.functions.len());
        self.functions.push(info);
    }

    /// The type of `self` in `fun Type.name(...)`: a structure, or a basic type (§12.4).
    fn receiver_type(&mut self, receiver: &ast::Ident) -> Option<Type> {
        if let Some(&index) = self.tables.struct_names.get(&receiver.name) {
            return Some(Type::Struct(self.structs[index].id));
        }
        let ty = ast::TypeExpr {
            kind: ast::TypeExprKind::Named { module: Vec::new(), name: receiver.clone(), args: Vec::new() },
            span: receiver.span,
        };
        if receiver.name == "Domain" {
            self.not_implemented(receiver.span, "methods of generic types", "§12.4, §15");
            return None;
        }
        self.resolve_type(&ty)
    }

    /// `fun Pair.swap()`: a method of a generic structure, given to each of its instances,
    /// those made so far and those to come (§12.4, §15.3, C90).
    fn register_template_method(&mut self, template: usize, decl: Rc<ast::FunDecl>) {
        let name = &decl.name.name;
        let structure = self.generic_structs[template].decl;
        if let Some(previous) =
            self.generic_structs[template].methods.iter().find(|method| &method.name.name == name)
        {
            self.diagnostics.push(
                Diagnostic::error(format!("the method `{}.{name}` is already declared", structure.name.name))
                    .with_primary(decl.name.span, "declared again here")
                    .with_secondary(previous.name.span, "first declared here"),
            );
            return;
        }
        let field = structure.lines.iter().find_map(|line| match line {
            ast::StructLine::Field(field) if &field.name.name == name => Some(field.name.span),
            _ => None,
        });
        if let Some(field) = field {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` already has a field `{name}`", structure.name.name))
                    .with_primary(decl.name.span, "")
                    .with_secondary(field, "field declared here")
                    .with_help("give the method another name"),
            );
            return;
        }
        self.generic_structs[template].methods.push(Rc::clone(&decl));
        let mut instances: Vec<usize> = self.generic_structs[template].instances.values().copied().collect();
        instances.sort_unstable();
        for index in instances {
            self.register_method_instance(Rc::clone(&decl), template, index);
        }
    }

    /// The method `decl` of a generic structure, for its instance `index`.
    pub(crate) fn register_method_instance(&mut self, decl: Rc<ast::FunDecl>, template: usize, index: usize) {
        let module = self.generic_structs[template].module;
        let ty = Type::Struct(self.structs[index].id);
        let function = self.functions.len();
        self.methods.entry((ty, decl.name.name.clone())).or_default().push(function);
        let equals = decl.name.name == "equals";
        self.functions.push(FunctionInfo {
            decl,
            module,
            prefix: self.qualified_in(module, ""),
            receiver: Some(ty),
            own_method: true,
            collection: None,
            struct_args: self.structs[index].type_args.clone(),
            implicit_self: false,
            var_self: false,
            native: None,
            foreign: None,
            closure: None,
            expected: None,
            type_params: Vec::new(),
            test: None,
            signature: None,
            declared_ret: None,
            modifies: Vec::new(),
            instances: HashMap::new(),
            instance: None,
        });
        // Before the pass over the signatures, that pass resolves it.
        if !self.signatures_started {
            return;
        }
        let previous = self.enter_module(module);
        self.resolve_signature(function);
        self.enter_module(previous);
        let info = &self.functions[function];
        if info.signature.is_some() && !info.is_generic() {
            let ret = info.declared_ret.map_or(Ret::Unknown, Ret::Declared);
            let instance = self.new_instance(function, Vec::new(), None, ret);
            self.functions[function].instance = Some(instance);
        }
        if equals && self.equalities_registered {
            self.register_equality(function);
        }
    }

    /// A method of a collection has its own name among the methods of the collection in
    /// its file, and among the operations of the language (C97).
    fn check_collection_method_name(&mut self, kind: &'static str, decl: &ast::FunDecl) -> bool {
        let name = &decl.name.name;
        let previous = self.collection_methods.get(&(kind, name.clone())).and_then(|methods| {
            methods.iter().copied().find(|&method| self.functions[method].module == self.module)
        });
        if let Some(previous) = previous {
            self.diagnostics.push(
                Diagnostic::error(format!("the method `{kind}.{name}` is already declared"))
                    .with_primary(decl.name.span, "declared again here")
                    .with_secondary(self.functions[previous].decl.name.span, "first declared here"),
            );
            return false;
        }
        let standard: &[&str] = match kind {
            "List" => &["size", "first", "last", "add"],
            "Set" => &["size", "add", "remove"],
            _ => &["size", "get", "remove"],
        };
        if standard.contains(&name.as_str()) {
            self.diagnostics.push(
                Diagnostic::error(format!("`{kind}` already has `{name}`"))
                    .with_primary(decl.name.span, "")
                    .with_help("give the method another name"),
            );
            return false;
        }
        true
    }

    /// A method has its own name among the methods and the fields of its type.
    fn check_method_name(&mut self, ty: Type, decl: &ast::FunDecl) -> bool {
        let name = &decl.name.name;
        let same_module = self.methods.get(&(ty, name.clone())).and_then(|methods| {
            methods.iter().copied().find(|&method| self.functions[method].module == self.module)
        });
        if let Some(previous) = same_module {
            self.diagnostics.push(
                Diagnostic::error(format!("the method `{ty}.{name}` is already declared"))
                    .with_primary(decl.name.span, "declared again here")
                    .with_secondary(self.functions[previous].decl.name.span, "first declared here"),
            );
            return false;
        }
        if let Type::Struct(structure) = ty
            && let Some(field) =
                self.structs[self.struct_index(structure)].fields.iter().find(|field| &field.name == name)
        {
            self.diagnostics.push(
                Diagnostic::error(format!("`{ty}` already has a field `{name}`"))
                    .with_primary(decl.name.span, "")
                    .with_secondary(field.span, "field declared here")
                    .with_help("give the method another name"),
            );
            return false;
        }
        let standard = match ty {
            Type::Text => ["size"].as_slice(),
            Type::Error => ["message"].as_slice(),
            _ => &[],
        };
        if standard.contains(&name.as_str()) {
            self.diagnostics.push(
                Diagnostic::error(format!("`{ty}` already has `{name}`"))
                    .with_primary(decl.name.span, "")
                    .with_help("give the method another name"),
            );
            return false;
        }
        true
    }

    pub(crate) fn new_instance(
        &mut self,
        function: usize,
        arg_types: Vec<Type>,
        origin: Option<Span>,
        ret: Ret,
    ) -> usize {
        self.instances.push(Instance {
            function,
            synthetic: None,
            arg_types,
            origin,
            ret,
            bindings: HashMap::new(),
            state: BodyState::Unchecked,
            reads: Vec::new(),
            calls: Vec::new(),
            checked: None,
        });
        self.instances.len() - 1
    }

    /// The types of the parameters, the return type and the `modifies` clause (§11.1).
    pub(crate) fn resolve_signature(&mut self, index: usize) {
        let decl = self.functions[index].decl.clone();
        let mut supported = true;
        // `a name b` gives two values: `a` and `b`, or `self` and `b` for a method (§9.5, D3).
        let explicit = decl.params.iter().filter(|param| param.name.name != "self").count();
        let expected = if decl.receiver.is_some() { 1 } else { 2 };
        if decl.infix && explicit != expected {
            let what = if decl.receiver.is_some() {
                "an `infix` method takes one value"
            } else {
                "an `infix` function takes two values"
            };
            self.diagnostics.push(
                Diagnostic::error(what)
                    .with_primary(decl.name.span, "")
                    .with_note("it is written between its two operands: `u dot v` (§9.5)"),
            );
            supported = false;
        }
        // In a method of an instance of a generic structure, its type parameters are the
        // types of the instance (C90); a method sees no other type variable.
        let struct_args = self.functions[index].struct_args.clone();
        let saved_vars =
            (!struct_args.is_empty()).then(|| std::mem::replace(&mut self.type_vars, struct_args));
        // `T in Comparable`, `T in Type`: the type variables of the signature (§15.2).
        let outer_vars = self.type_vars.len();
        let mut type_params = Vec::new();
        for (name, constraint) in &decl.type_params {
            let var = ir::VarRef::new(&name.name);
            let constraint = match &constraint.kind {
                ast::TypeExprKind::Named { module, name: set, args }
                    if module.is_empty() && args.is_empty() && set.name == "Type" =>
                {
                    None
                }
                _ => match self.resolve_type(constraint) {
                    Some(trait_type @ Type::Trait(_)) => Some(trait_type),
                    Some(other) => {
                        self.diagnostics.push(
                            Diagnostic::error(format!("`{other}` is not a trait"))
                                .with_primary(constraint.span, "")
                                .with_note("a type variable is declared with a trait, or `Type` for every type (§15.2)"),
                        );
                        supported = false;
                        None
                    }
                    None => {
                        supported = false;
                        None
                    }
                },
            };
            type_params.push((var, constraint, name.span));
            self.type_vars.push((name.name.clone(), Type::Var(var)));
        }
        if let Some(foreign) = &decl.foreign {
            supported &= self.foreign_function(index, foreign);
        }
        // The type of `self` in a method of a collection: its first type variables are the
        // types of the elements, of the keys and of the values (C97).
        if let Some(kind) = self.functions[index].collection {
            let vars: Vec<Type> = type_params.iter().map(|&(var, _, _)| Type::Var(var)).collect();
            let receiver = match (kind, vars.as_slice()) {
                ("List", [element, ..]) => Some(Type::list(*element)),
                ("Set", [element, ..]) => Some(Type::set(*element)),
                ("Map", [key, value, ..]) => Some(Type::map(*key, *value)),
                _ => None,
            };
            if receiver.is_none() {
                let example = match kind {
                    "Map" => "fun Map.keys_list() in List of K, K in Type, V in Type",
                    "Set" => "fun Set.any_element() in maybe T, T in Type",
                    _ => "fun List.second() in maybe T, T in Type",
                };
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "a method of `{kind}` declares the type{} of its elements",
                        if kind == "Map" { "s" } else { "" }
                    ))
                    .with_primary(decl.name.span, "")
                    .with_note("its first type variables are the types of the elements (C97)")
                    .with_help(format!("write it: `{example}`")),
                );
                supported = false;
            }
            self.functions[index].receiver = receiver;
        }
        let mut params = Vec::new();
        // `self` is the first parameter of a method: written `var self` when the method
        // changes it, left out otherwise (§12.4).
        if let Some(receiver) = self.functions[index].receiver {
            let var_self = decl.params.first().is_some_and(|param| param.name.name == "self");
            if !var_self {
                self.functions[index].implicit_self = true;
            }
            self.functions[index].var_self = var_self;
            let span =
                decl.params.first().filter(|_| var_self).map_or(decl.name.span, |param| param.name.span);
            params.push(ParamInfo {
                name: "self".to_string(),
                ty: Some(receiver),
                by_reference: var_self,
                has_default: false,
                span,
            });
        }
        let mut defaults_started = false;
        for (position, param) in decl.params.iter().enumerate() {
            if let Some(previous) = decl.params[..position].iter().find(|p| p.name.name == param.name.name) {
                self.diagnostics.push(
                    Diagnostic::error(format!("the parameter `{}` is declared twice", param.name.name))
                        .with_primary(param.name.span, "")
                        .with_secondary(previous.name.span, "first declared here"),
                );
                supported = false;
            }
            if param.name.name == "self" && self.functions[index].receiver.is_none() {
                self.diagnostics.push(
                    Diagnostic::error("`self` is a parameter of methods only")
                        .with_primary(param.name.span, "")
                        .with_note("a method is declared `fun Type.name(...)` (§12.4)"),
                );
                supported = false;
                continue;
            }
            if param.name.name == "self" {
                let problem = if param.var.is_none() {
                    Some(
                        "`self` is written in the parameters only as `var self`, for a method that changes its object",
                    )
                } else if position > 0 {
                    Some("`var self` is the first parameter")
                } else if param.ty.is_some() || param.default.is_some() {
                    Some("`var self` has the type of the method, and no default value")
                } else {
                    None
                };
                if let Some(problem) = problem {
                    self.diagnostics.push(
                        Diagnostic::error(problem)
                            .with_primary(param.name.span, "")
                            .with_note("a method reads its object as `self`; it changes it only when declared `fun Type.name(var self, ...)` (§12.4)"),
                    );
                    supported = false;
                }
                continue;
            }
            if let (Some(var_span), Some(default)) = (param.var, &param.default) {
                self.diagnostics.push(
                    Diagnostic::error("a `var` parameter cannot have a default value")
                        .with_primary(default.span, "")
                        .with_secondary(var_span, "this parameter works on a variable of the caller")
                        .with_note("the caller always gives the variable of a `var` parameter (§11.2)"),
                );
                supported = false;
            }
            if param.default.is_some() {
                defaults_started = true;
            } else if defaults_started {
                self.diagnostics.push(
                    Diagnostic::error(format!("the parameter `{}` needs a default value", param.name.name))
                        .with_primary(param.name.span, "")
                        .with_note("parameters with a default value come last (§11.2)"),
                );
                supported = false;
            }
            let ty = match &param.ty {
                Some(written) => match self.resolve_type(written) {
                    Some(ty) => Some(ty),
                    None => {
                        supported = false;
                        None
                    }
                },
                // Without a type, the parameter takes the type of each call's argument (C1).
                None => None,
            };
            params.push(ParamInfo {
                name: param.name.name.clone(),
                ty,
                by_reference: param.var.is_some(),
                has_default: param.default.is_some(),
                span: param.name.span,
            });
        }
        let mut declared_ret = match &decl.ret {
            Some(ty) => {
                let ty = self.resolve_type(ty);
                supported &= ty.is_some();
                ty
            }
            None => None,
        };
        // An anonymous function takes the types of the function type expected there.
        if let Some(expected) = self.functions[index].expected.clone() {
            for (param, ty) in params.iter_mut().zip(&expected.params) {
                param.ty.get_or_insert(*ty);
            }
            declared_ret.get_or_insert(expected.ret);
        }
        // The variables that a nested function shares are not globals (§11.5).
        let shared: Vec<String> = match &self.functions[index].closure {
            Some(closure) => {
                closure.captures.iter().filter(|c| c.by_reference).map(|c| c.name.clone()).collect()
            }
            None => Vec::new(),
        };
        let modifies = decl
            .modifies
            .iter()
            .filter(|name| !shared.contains(&name.name))
            .filter_map(|name| self.modified_global(name))
            .collect();
        self.type_vars.truncate(outer_vars);
        if let Some(saved) = saved_vars {
            self.type_vars = saved;
        }
        let function = &mut self.functions[index];
        function.declared_ret = declared_ret;
        function.modifies = modifies;
        function.type_params = type_params;
        function.signature = supported.then_some(params);
    }

    /// A name after `modifies`: a `var` of the script, declared once.
    fn modified_global(&mut self, name: &ast::Ident) -> Option<ir::LocalId> {
        let Some(global) = self.tables.globals.get(&name.name) else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is not a variable of the script", name.name))
                    .with_primary(name.span, "")
                    .with_note(
                        "`modifies` names the variables of the script that a function changes (§11.5)",
                    ),
            );
            return None;
        };
        let (local, decl) = (global.local, global.decl);
        if !decl.mutable {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is a constant", name.name))
                    .with_primary(name.span, "")
                    .with_secondary(decl.name.span, "declared with `let`")
                    .with_help("declare it with `var` for a function to change it"),
            );
            return None;
        }
        if local.is_none() {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "`{}` is declared several times at the top level of the script",
                    name.name
                ))
                .with_primary(name.span, "a function cannot tell which declaration this is")
                .with_help("give the top-level declarations different names"),
            );
        }
        local
    }

    /// Checks the bodies that no call required earlier, so that their errors are
    /// reported. A generic function is checked for the types it is called with: one
    /// that is never called cannot be checked, which a warning says.
    pub(crate) fn check_remaining_functions(&mut self) {
        let mut index = 0;
        while index < self.instances.len() {
            self.check_instance(index);
            index += 1;
        }
        for function in &self.functions {
            // The functions of the standard library are checked by its own tests.
            let standard = self.modules[function.module].standard;
            let nested = function.closure.is_some();
            if function.is_generic() && function.instances.is_empty() && !standard && !nested {
                let name = &function.decl.name.name;
                let note = if function.decl.type_params.is_empty() {
                    "a function with parameters without a type is checked for the types of each call (§15.3)"
                } else {
                    "a function with type variables is checked for the types of each call (§15.3)"
                };
                self.diagnostics.push(
                    Diagnostic::new(
                        lion_diagnostics::Severity::Warning,
                        format!("`{name}` is never called, so it is not checked"),
                    )
                    .with_primary(function.decl.name.span, "")
                    .with_note(note),
                );
            }
        }
    }

    fn check_instance(&mut self, instance: usize) {
        if self.instances[instance].state != BodyState::Unchecked {
            return;
        }
        let function = self.instances[instance].function;
        let params = self.functions[function].signature.clone().expect("an instance has a signature");
        self.instances[instance].state = BodyState::Checking;
        if let Ret::Unknown = self.instances[instance].ret {
            self.instances[instance].ret = Ret::Inferring;
        }
        let first_diagnostic = self.diagnostics.len();
        let previous = self.enter_module(self.functions[function].module);
        let interrupted = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Function(instance)));
        // In the body, a type variable is the type it takes for this instance (§15.2).
        let mut bindings = self.functions[function].struct_args.clone();
        bindings.extend(self.instances[instance].bindings.iter().map(|(var, ty)| (var.name(), *ty)));
        let outer_vars = std::mem::replace(&mut self.type_vars, bindings);
        let (body, defaults, param_types) = self.function_body(instance, &params);
        self.type_vars = outer_vars;
        let ctx = std::mem::replace(&mut self.ctx, interrupted);
        self.enter_module(previous);
        // An error in a generic function shows which call created the instance.
        if let Some(origin) = self.instances[instance].origin {
            let name = &self.functions[function].decl.name.name;
            let types: Vec<String> = param_types.iter().map(Type::to_string).collect();
            let label = format!("`{name}` is checked for ({}) because of this call", types.join(", "));
            // An error in the standard library is shown at the call of the program (C64).
            let at_call = self.modules[self.functions[function].module].standard
                && !self.is_standard_source(origin.source);
            for diagnostic in &mut self.diagnostics[first_diagnostic..] {
                let call = lion_diagnostics::Label { span: origin, message: label.clone(), primary: at_call };
                if at_call {
                    diagnostic.labels.iter_mut().for_each(|label| label.primary = false);
                    diagnostic.labels.insert(0, call);
                } else {
                    diagnostic.labels.push(call);
                }
            }
        }
        let checked = &mut self.instances[instance];
        checked.reads = ctx.reads;
        checked.calls = ctx.calls;
        let captures =
            self.functions[function].closure.as_ref().map_or(0, |closure| closure.captures.len() as u32);
        let checked = &mut self.instances[instance];
        checked.checked =
            Some(CheckedBody { params: param_types, captures, locals: ctx.locals, body, defaults });
        checked.state = BodyState::Checked;
    }

    /// The body of an instance, its default values, and the types of its parameters.
    fn function_body(
        &mut self,
        instance: usize,
        params: &[ParamInfo],
    ) -> (Vec<ir::Stmt>, Vec<(u32, ir::Expr)>, Vec<Type>) {
        let function = self.instances[instance].function;
        let decl = self.functions[function].decl.clone();
        let arg_types = self.instances[instance].arg_types.clone();
        let generic = self.functions[function].is_generic();
        // The written parameters, after the `self` of a method that does not write it.
        let offset = usize::from(self.functions[function].implicit_self);
        // The parameters are the first locals (C6: they belong to the outermost block).
        let ids: Vec<ir::LocalId> = params
            .iter()
            .enumerate()
            .map(|(position, param)| {
                let ty = match param.ty {
                    Some(ty) if ty.has_vars() => arg_types.get(position).copied(),
                    ty => ty.or_else(|| arg_types.get(position).copied()),
                };
                let id = self.push_local(LocalInfo {
                    name: param.name.clone(),
                    ty,
                    mutable: param.by_reference,
                    temporary: false,
                    by_reference: param.by_reference,
                    decl_span: param.span,
                    initialized: true,
                    first_assignment: None,
                    loop_variable: false,
                    captured: false,
                    boxed: false,
                    shared: None,
                });
                self.ctx.flow.set(id, Assigned::Yes);
                id
            })
            .collect();
        // The variables captured by a function declared inside a body follow its
        // parameters; it sees its own name, to call itself (§11.5).
        let captures = self.functions[function].closure.as_ref().map(|closure| closure.captures.clone());
        for capture in captures.iter().flatten() {
            let id = self.push_local(LocalInfo {
                name: capture.name.clone(),
                ty: Some(capture.ty),
                mutable: capture.by_reference,
                temporary: false,
                by_reference: false,
                decl_span: capture.span,
                initialized: true,
                first_assignment: None,
                loop_variable: false,
                captured: true,
                boxed: capture.by_reference,
                shared: capture.sharing,
            });
            self.ctx.flow.set(id, Assigned::Yes);
            let scope = self.ctx.scopes.first_mut().expect("the outermost block");
            scope.names.insert(capture.name.clone(), id);
        }
        if captures.is_some() && decl.name.name != "fun" {
            let scope = self.ctx.scopes.first_mut().expect("the outermost block");
            scope.functions.insert(decl.name.name.clone(), function);
        }
        // A default value sees the globals and the parameters before it (C10). In a
        // generic instance, only the omitted arguments need it, and a parameter without
        // a type takes the type of its default value.
        let mut defaults = Vec::new();
        let mut valid = true;
        for (position, (param, id)) in params.iter().zip(&ids).enumerate() {
            let needed = !generic || position >= arg_types.len();
            let written = position.checked_sub(offset).map(|position| &decl.params[position]);
            if let (true, Some(default)) = (needed, written.and_then(|param| param.default.as_ref())) {
                match self.expr(default) {
                    Some(value) => {
                        let expected = self.ctx.locals[id.index()].ty.unwrap_or(value.ty);
                        self.ctx.locals[id.index()].ty = Some(expected);
                        let context = (param.span, "parameter declared here".to_string());
                        match self.coerce(value, expected, Some(context)) {
                            Some(value) => defaults.push((position as u32, value)),
                            None => valid = false,
                        }
                    }
                    None => valid = false,
                }
            }
            let scope = self.ctx.scopes.first_mut().expect("the outermost block");
            scope.names.insert(param.name.clone(), *id);
        }
        let param_types: Vec<Type> =
            ids.iter().map(|id| self.ctx.locals[id.index()].ty.unwrap_or(Type::None)).collect();
        if !valid {
            self.instances[instance].ret = Ret::Failed;
            return (Vec::new(), defaults, param_types);
        }
        let mut body = match &decl.body {
            ast::FunBody::Block(block) => self.stmts(&block.stmts),
            // `fun f(x) = expr` returns `expr` (§11.1).
            ast::FunBody::Expr(value) => {
                self.return_in_function(instance, Some(value), value.span).into_iter().collect()
            }
            ast::FunBody::Required => unreachable!("a required method of a trait has no instance"),
            // The implementation computes the value from the parameters.
            ast::FunBody::Foreign => {
                let builtin = match self.functions[function].foreign {
                    Some((index, pure)) => ir::Builtin::Foreign { index, pure },
                    None => ir::Builtin::Native(
                        self.functions[function].native.expect("a checked foreign function is native"),
                    ),
                };
                let args = ids
                    .iter()
                    .zip(&param_types)
                    .map(|(id, ty)| typed(ir::ExprKind::Local(*id), *ty, decl.name.span))
                    .collect();
                let ret = self.declared_return(instance).unwrap_or(Type::None);
                let kind = ir::ExprKind::CallBuiltin { builtin, args };
                self.ctx.flow = crate::flow::Flow::unreachable();
                vec![ir::Stmt::Return(Some(typed(kind, ret, decl.name.span)))]
            }
        };
        // A function of a module gives the globals of its module their values first (D81).
        let module = self.functions[function].module;
        if module > 0 && self.modules[module].init.is_some() {
            body.insert(0, ir::Stmt::InitModule { module: module as u32 });
        }
        let falls_through = self.ctx.flow.is_reachable();
        self.close_scope();
        let ret = self.settle_return_type(instance);
        if falls_through && ret.is_some_and(|ty| ty != Type::None) {
            let name = &decl.name.name;
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` does not return a value on every path"))
                    .with_primary(decl.name.span, "this function can reach its end without `return`")
                    .with_note(
                        "when a `return` of a function gives a value, every path must end with one (§11.4)",
                    ),
            );
        }
        if let Some(ret) = ret
            && ret.members().contains(&Type::Float)
            && !ret.members().contains(&Type::Int)
        {
            widen_returns(&mut body);
        }
        (body, defaults, param_types)
    }

    /// The declared return type, or the one inferred from the `return` statements (C12).
    fn settle_return_type(&mut self, instance: usize) -> Option<Type> {
        match self.instances[instance].ret {
            Ret::Declared(ty) => return Some(ty),
            Ret::Inferring => {}
            _ => return None,
        }
        if self.ctx.failed_return {
            self.instances[instance].ret = Ret::Failed;
            return None;
        }
        let decl = self.functions[self.instances[instance].function].decl.clone();
        let returns = std::mem::take(&mut self.ctx.returns);
        let values: Vec<(Type, Span)> =
            returns.iter().filter_map(|&(ty, span)| ty.map(|ty| (ty, span))).collect();
        let ret = if values.is_empty() {
            Some(Type::None)
        } else if let Some(&(_, bare)) = returns.iter().find(|(ty, _)| ty.is_none()) {
            let name = &decl.name.name;
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "this `return` has no value, but other returns of `{name}` give one"
                ))
                .with_primary(bare, "")
                .with_secondary(values[0].1, "a value is returned here")
                .with_note(
                    "when a `return` of a function gives a value, every path must end with one (§11.4)",
                ),
            );
            None
        } else {
            // The union of the returned types; an Int joins a Float when both appear (§8.5).
            let union = Type::union(values.iter().map(|(ty, _)| *ty));
            let members = union.members();
            if members.contains(&Type::Int) && members.contains(&Type::Float) {
                Some(Type::union(members.into_iter().filter(|member| *member != Type::Int)))
            } else {
                Some(union)
            }
        };
        self.instances[instance].ret = ret.map_or(Ret::Failed, Ret::Inferred);
        ret
    }

    /// The return type written in the declaration of an instance's function.
    pub(crate) fn declared_return(&self, instance: usize) -> Option<Type> {
        match self.instances[instance].ret {
            Ret::Declared(ty) => Some(ty),
            _ => None,
        }
    }

    /// The return type of an instance, checking its body first if it is inferred.
    pub(crate) fn return_type_of(&mut self, instance: usize, call: Span) -> Option<Type> {
        if let Ret::Unknown = self.instances[instance].ret {
            self.demands.push(call);
            self.check_instance(instance);
            self.demands.pop();
        }
        match self.instances[instance].ret {
            Ret::Declared(ty) | Ret::Inferred(ty) => Some(ty),
            Ret::Inferring => {
                let decl = self.functions[self.instances[instance].function].decl.clone();
                let name = &decl.name.name;
                self.diagnostics.push(
                    Diagnostic::error(format!("the return type of `{name}` must be written"))
                        .with_primary(
                            call,
                            format!("`{name}` is called here before its return type is known"),
                        )
                        .with_secondary(decl.name.span, "declared here")
                        .with_help(format!("write it after the parameters: `fun {name}(...) in Type`")),
                );
                None
            }
            Ret::Unknown | Ret::Failed => None,
        }
    }

    /// `return` inside a function (§11.4).
    pub(crate) fn return_in_function(
        &mut self,
        instance: usize,
        value: Option<&ast::Expr>,
        span: Span,
    ) -> Option<ir::Stmt> {
        let declared = match self.instances[instance].ret {
            Ret::Declared(ty) => Some(ty),
            _ => None,
        };
        let decl = self.functions[self.instances[instance].function].decl.clone();
        let stmt = match value {
            None => {
                if let Some(ty) = declared.filter(|ty| *ty != Type::None) {
                    let name = &decl.name.name;
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "`{name}` returns {}: this `return` needs a value",
                            article(ty)
                        ))
                        .with_primary(span, ""),
                    );
                }
                self.ctx.returns.push((None, span));
                Some(ir::Stmt::Return(None))
            }
            Some(expr) => {
                let checked = match declared {
                    Some(ty) => self.expr_expecting(expr, ty),
                    None => self.expr(expr),
                };
                let value = match (checked, declared) {
                    (Some(checked), Some(ty)) => {
                        let context =
                            decl.ret.as_ref().map(|ret| (ret.span, "return type declared here".to_string()));
                        self.coerce(checked, ty, context)
                    }
                    (Some(checked), None) => {
                        self.ctx.returns.push((Some(checked.ty), expr.span));
                        Some(checked)
                    }
                    (None, _) => {
                        self.ctx.failed_return = true;
                        None
                    }
                };
                value.map(|value| ir::Stmt::Return(Some(value)))
            }
        };
        self.ctx.flow = crate::flow::Flow::unreachable();
        stmt
    }

    /// A call of a function of the file (§11.2).
    pub(crate) fn call_function(
        &mut self,
        index: usize,
        callee: Span,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        if !self.check_c_call(index, callee) {
            for arg in args {
                self.expr(&arg.value);
            }
            return None;
        }
        self.call_with(index, None, Vec::new(), callee, args, span)
    }

    /// A call of a function, or of a method with its object already checked; `after`
    /// runs once the call returns (§12.3).
    pub(crate) fn call_with(
        &mut self,
        index: usize,
        receiver: Option<Pending>,
        after: Vec<ir::Stmt>,
        callee: Span,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<ir::Expr> {
        let name = self.functions[index].decl.name.name.clone();
        let Some(mut params) = self.functions[index].signature.clone() else {
            for arg in args {
                self.expr(&arg.value);
            }
            return None;
        };
        let mut pending = Vec::new();
        let has_receiver = receiver.is_some();
        if let Some(receiver) = receiver {
            params.remove(0);
            pending.push(Some(receiver));
        }
        let required = params.iter().filter(|param| !param.has_default).count();
        if args.len() > params.len() || args.is_empty() && required > 0 {
            let expected = if required == params.len() {
                format!("{required}")
            } else {
                format!("{required} to {}", params.len())
            };
            let plural = if params.len() == 1 { "" } else { "s" };
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` takes {expected} argument{plural}, not {}", args.len()))
                    .with_primary(span, "")
                    .with_secondary(self.functions[index].decl.name.span, "declared here"),
            );
            for arg in args {
                self.expr(&arg.value);
            }
            return None;
        }
        if args.len() < required && !has_receiver {
            // `f(1)`: the function that waits for the other arguments (§11.3).
            let value = self.function_value(index, callee, None)?;
            let Type::Fun(data) = value.ty else { unreachable!("a function value") };
            return self.partial(value, &data.get(), args, span);
        }
        for (arg, param) in args.iter().zip(&params) {
            pending.push(self.argument(&name, arg, param));
        }
        // A function declared inside a body calls itself with the variables it captured.
        if let Some(closure) = self.functions[index].closure.clone() {
            for capture in &closure.captures {
                let local = self.lookup(&capture.name).expect("the captures are visible in the function");
                let kind =
                    if capture.by_reference { ir::ExprKind::Cell(local) } else { ir::ExprKind::Local(local) };
                pending.push(Some(Pending::Value(typed(kind, capture.ty, span))));
            }
        }
        let pending: Vec<Pending> = pending.into_iter().collect::<Option<_>>()?;
        let instance = match self.functions[index].instance {
            Some(instance) => instance,
            None => self.instantiate(index, pending.iter().map(Pending::ty).collect(), callee)?,
        };
        let ret = self.return_type_of(instance, callee)?;
        self.ctx.calls.push(instance);
        self.record_early_call(instance, span);
        if self.ctx.kind == ContextKind::Script {
            // The function may change the `var` globals: what was known of them is forgotten.
            let globals: Vec<ir::LocalId> = self
                .tables
                .globals
                .values()
                .filter(|global| global.decl.mutable)
                .filter_map(|global| global.local)
                .collect();
            for global in globals {
                self.ctx.flow.narrow(global, None);
            }
        }
        Some(self.build_call(instance, pending, after, ret, span))
    }

    /// The instance of a generic function for the types of the given arguments (C1).
    pub(crate) fn instantiate(&mut self, function: usize, arg_types: Vec<Type>, call: Span) -> Option<usize> {
        if let Some(&instance) = self.functions[function].instances.get(&arg_types) {
            return Some(instance);
        }
        let bindings = self.bind_type_variables(function, &arg_types, call)?;
        let ret = match self.functions[function].declared_ret {
            Some(ret) => Ret::Declared(self.make_applied(ret.substitute(&bindings), call)?),
            None => Ret::Unknown,
        };
        let instance = self.new_instance(function, arg_types.clone(), Some(call), ret);
        self.instances[instance].bindings = bindings;
        self.functions[function].instances.insert(arg_types, instance);
        Some(instance)
    }

    /// The types that the type variables take for these arguments, which must fit the
    /// parameters, and satisfy the traits of the variables (§15.2).
    fn bind_type_variables(
        &mut self,
        function: usize,
        arg_types: &[Type],
        call: Span,
    ) -> Option<HashMap<ir::VarRef, Type>> {
        let mut bindings = HashMap::new();
        let params = self.functions[function].signature.clone().unwrap_or_default();
        for (param, &actual) in params.iter().zip(arg_types) {
            let Some(pattern) = param.ty.filter(|ty| ty.has_vars()) else { continue };
            if !self.unify_param(pattern, actual, &mut bindings, call) {
                let mut error = Diagnostic::error(format!(
                    "the parameter `{}` is {}, which does not fit {}",
                    param.name,
                    pattern,
                    article(actual)
                ))
                .with_primary(call, "")
                .with_secondary(param.span, "parameter declared here");
                if let Type::Var(var) = pattern
                    && let Some(bound) = bindings.get(&var)
                {
                    error = error.with_note(format!("`{}` is already {bound} for this call", var.name()));
                }
                self.diagnostics.push(error);
                return None;
            }
        }
        for &(var, constraint, span) in &self.functions[function].type_params.clone() {
            let (Some(constraint), Some(&bound)) = (constraint, bindings.get(&var)) else { continue };
            if !bound.is_subset_of(constraint) {
                self.diagnostics.push(
                    Diagnostic::error(format!("{} is not {constraint}", crate::capitalize(&article(bound))))
                        .with_primary(call, "")
                        .with_secondary(span, format!("`{}` must be {constraint}", var.name()))
                        .with_note(format!(
                            "the type variable `{}` takes the type of the argument (§15.2)",
                            var.name()
                        )),
                );
                return None;
            }
        }
        Some(bindings)
    }

    fn argument(&mut self, function: &str, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        let mut valid = true;
        if let Some(name) = &arg.name
            && name.name != param.name
        {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "this argument is named `{}`, but the parameter at this position is `{}`",
                    name.name, param.name
                ))
                .with_primary(name.span, "")
                .with_secondary(param.span, "parameter declared here")
                .with_note("named arguments keep the order of the parameters (§11.2)"),
            );
            valid = false;
        }
        if let Some(var_span) = arg.var_marker
            && !param.by_reference
        {
            self.diagnostics.push(
                Diagnostic::error(format!("the parameter `{}` of `{function}` is not `var`", param.name))
                    .with_primary(var_span, "remove this `var`")
                    .with_secondary(param.span, "parameter declared here")
                    .with_note("`var` at a call marks an argument that the function modifies (§11.2)"),
            );
            valid = false;
        }
        let checked = if param.by_reference {
            self.reference_argument(arg, param)
        } else {
            self.value_argument(arg, param)
        };
        checked.filter(|_| valid)
    }

    fn value_argument(&mut self, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        // A type variable takes the type of the argument (§15.2).
        if param.ty.is_some_and(Type::has_vars) {
            return self.expr(&arg.value).map(Pending::Value);
        }
        let value = match param.ty {
            Some(ty) => self.expr_expecting(&arg.value, ty)?,
            None => self.expr(&arg.value)?,
        };
        match param.ty {
            Some(ty) => {
                let context = (param.span, "parameter declared here".to_string());
                Some(Pending::Value(self.coerce(value, ty, Some(context))?))
            }
            None => Some(Pending::Value(value)),
        }
    }

    /// The argument of a `var` parameter: a `var` variable, which the call may change,
    /// or any other value, whose change is lost (§11.2).
    fn reference_argument(&mut self, arg: &ast::Arg, param: &ParamInfo) -> Option<Pending> {
        if is_part_of_variable(&arg.value) {
            let path = self.place_path(&arg.value)?;
            if let Some(expected) = param.ty
                && path.ty() != expected
            {
                self.diagnostics.push(
                    Diagnostic::error("mismatched types")
                        .with_primary(arg.value.span, format!("this is {}", article(path.ty())))
                        .with_secondary(param.span, format!("this `var` parameter is {}", article(expected)))
                        .with_note("a `var` parameter works on the caller's variable itself, so both have the same type (§11.2)"),
                );
                return None;
            }
            let (setup, path) = self.stabilize(path);
            return Some(Pending::Element(path, setup));
        }
        let ast::ExprKind::Name(name) = &arg.value.kind else {
            let value = self.expr(&arg.value)?;
            let value = match param.ty {
                Some(ty) => {
                    self.coerce(value, ty, Some((param.span, "parameter declared here".to_string())))?
                }
                None => value,
            };
            return Some(Pending::Temporary(value));
        };
        let span = arg.value.span;
        let (place, ty, mutable, decl_span) = match self.resolve(name, span) {
            Resolved::Local(local) => {
                // An argument of a `var` parameter must have a value (C8).
                if !self.check_has_value(local, span)
                    || self.changes_outside_parallel(crate::parallel::Variable::Local(local), name, span)
                {
                    return None;
                }
                // After the call, it may hold any value of its type.
                self.ctx.flow.narrow(local, None);
                let info = &self.ctx.locals[local.index()];
                (ir::Place::Local(local), info.ty?, info.mutable, info.decl_span)
            }
            Resolved::Global(local) => {
                // Giving a global to a `var` parameter changes it (C7).
                if !self.check_global_assignment(local, span)
                    || self.changes_outside_parallel(crate::parallel::Variable::Global(local), name, span)
                {
                    return None;
                }
                self.ctx.reads.push(local);
                let global = &self.global_info(local);
                let GlobalType::Known(ty) = global.ty else { return None };
                (ir::Place::Global(local), ty?, true, global.decl.name.span)
            }
            Resolved::Function(_) | Resolved::Standard(_) => {
                self.not_implemented(span, "functions used as values", "§11.3");
                return None;
            }
            Resolved::Nothing => return None,
        };
        if !mutable {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "the constant `{name}` cannot be given to the `var` parameter `{}`",
                    param.name
                ))
                .with_primary(span, "")
                .with_secondary(decl_span, "declared with `let`")
                .with_help(format!("declare `{name}` with `var`, or give a copy: `({name})`")),
            );
            return None;
        }
        if let Some(expected) = param.ty
            && ty != expected
        {
            self.diagnostics.push(
                Diagnostic::error("mismatched types")
                    .with_primary(span, format!("this is {}", article(ty)))
                    .with_secondary(param.span, format!("this `var` parameter is {}", article(expected)))
                    .with_note("a `var` parameter works on the caller's variable itself, so both have the same type (§11.2)"),
            );
            return None;
        }
        Some(Pending::Place(place, ty))
    }

    /// The call, keeping the left-to-right order of the arguments (§9.2) when some of
    /// them are stored in temporaries to be passed by reference. The parts of variables
    /// given to `var` parameters are stored back after the call, then `after` runs.
    fn build_call(
        &mut self,
        instance: usize,
        pending: Vec<Pending>,
        mut after: Vec<ir::Stmt>,
        ret: Type,
        span: Span,
    ) -> ir::Expr {
        let needs_temporaries =
            pending.iter().any(|arg| matches!(arg, Pending::Temporary(_) | Pending::Element(..)));
        let mut before = Vec::new();
        let mut stored = Vec::new();
        let mut args = Vec::new();
        for arg in pending {
            match arg {
                Pending::Value(value) if needs_temporaries => {
                    let temp = self.temporary(value.ty, value.span);
                    args.push(ir::Arg::Value(typed(ir::ExprKind::Local(temp), value.ty, value.span)));
                    before.push(ir::Stmt::Assign { place: ir::Place::Local(temp), value });
                }
                Pending::Value(value) => args.push(ir::Arg::Value(value)),
                Pending::Place(place, _) => args.push(ir::Arg::Reference(place)),
                Pending::Temporary(value) => {
                    let temp = self.temporary(value.ty, value.span);
                    args.push(ir::Arg::Reference(ir::Place::Local(temp)));
                    before.push(ir::Stmt::Assign { place: ir::Place::Local(temp), value });
                }
                Pending::Element(path, setup) => {
                    let temp = self.temporary(path.ty(), path.span);
                    before.extend(setup);
                    let value = self.read_path(&path, path.steps.len());
                    before.push(ir::Stmt::Assign { place: ir::Place::Local(temp), value });
                    args.push(ir::Arg::Reference(ir::Place::Local(temp)));
                    let value = typed(ir::ExprKind::Local(temp), path.ty(), path.span);
                    stored.push(ir::Stmt::AssignElement { root: path.root, path: path.steps.clone(), value });
                    stored.extend(self.check_invariants(&path, false));
                }
            }
        }
        stored.append(&mut after);
        let function = ir::FunctionId(instance as u32);
        let mut call = typed(ir::ExprKind::Call { function, args }, ret, span);
        if !stored.is_empty() {
            let result = self.temporary(ret, span);
            let value = Box::new(typed(ir::ExprKind::Local(result), ret, span));
            let rest = typed(ir::ExprKind::Block { stmts: stored, value }, ret, span);
            call = typed(
                ir::ExprKind::Let { local: result, value: Box::new(call), body: Box::new(rest) },
                ret,
                span,
            );
        }
        if !before.is_empty() {
            call = typed(ir::ExprKind::Block { stmts: before, value: Box::new(call) }, ret, span);
        }
        call
    }

    /// `foreign "lion" fun`: a function of the standard library that the implementation
    /// provides. Other foreign functions call C code (§21.2).
    fn foreign_function(&mut self, index: usize, foreign: &ast::Foreign) -> bool {
        let function = &self.functions[index];
        let decl = function.decl.clone();
        let module = &self.modules[function.module];
        if foreign.library != "lion" {
            return self.c_function(index, foreign);
        }
        let native = ir::Native::find(&module.name, &decl.name.name).filter(|_| module.standard);
        let Some(native) = native.filter(|_| decl.receiver.is_none()) else {
            self.diagnostics.push(
                Diagnostic::error(format!("`{}` is not a function that Lion provides", decl.name.name))
                    .with_primary(decl.name.span, "")
                    .with_note("`foreign \"lion\"` declares the functions of the standard library that the implementation provides (§23)"),
            );
            return false;
        };
        if decl.params.iter().any(|param| param.ty.is_none()) || decl.ret.is_none() {
            self.diagnostics.push(
                Diagnostic::internal(
                    "a foreign function writes the types of its parameters and of its value",
                )
                .with_primary(decl.name.span, ""),
            );
            return false;
        }
        self.functions[index].native = Some(native);
        true
    }

    /// A global that `instance` modifies, directly or through the instances it calls,
    /// and the instance that modifies it directly (§11.5).
    /// The globals that `instance` reads or modifies, directly or through its calls, each
    /// with the instance that uses it.
    pub(crate) fn globals_used_by(&self, instance: usize) -> Vec<(ir::LocalId, usize)> {
        let mut seen = HashSet::new();
        let mut queue = vec![instance];
        let mut used = Vec::new();
        while let Some(current) = queue.pop() {
            if !seen.insert(current) {
                continue;
            }
            let checked = &self.instances[current];
            used.extend(checked.reads.iter().map(|&global| (global, current)));
            if checked.synthetic.is_none() {
                let modifies = &self.functions[checked.function].modifies;
                used.extend(modifies.iter().map(|&global| (global, current)));
            }
            queue.extend(checked.calls.iter().copied());
        }
        used
    }

    /// A call of a builtin for which `found` holds, in the body of `instance` or of the
    /// functions it calls, with the instance that makes it.
    pub(crate) fn builtin_called_by(
        &self,
        instance: usize,
        found: &dyn Fn(ir::Builtin) -> bool,
    ) -> Option<(ir::Builtin, usize)> {
        let mut seen = HashSet::new();
        let mut queue = vec![instance];
        while let Some(current) = queue.pop() {
            if !seen.insert(current) {
                continue;
            }
            let checked = &self.instances[current];
            queue.extend(checked.calls.iter().copied());
            let Some(body) = &checked.checked else { continue };
            let mut builtin = None;
            ir::visit::exprs_in_stmts(&body.body, &mut |expr| {
                if let ir::ExprKind::CallBuiltin { builtin: called, .. } = expr.kind
                    && found(called)
                {
                    builtin.get_or_insert(called);
                }
            });
            if let Some(builtin) = builtin {
                return Some((builtin, current));
            }
        }
        None
    }

    pub(crate) fn modified_global_of(&self, instance: usize) -> Option<(ir::LocalId, Option<usize>)> {
        let mut seen = HashSet::new();
        let mut queue = vec![instance];
        while let Some(current) = queue.pop() {
            if !seen.insert(current) {
                continue;
            }
            let checked = &self.instances[current];
            // A `shared synced` object may change in parallel, a lock protecting it (§19.3).
            let synced =
                |global: &ir::LocalId| self.global_sharing.get(global).is_some_and(|sharing| sharing.synced);
            if checked.synthetic.is_none()
                && let Some(&global) =
                    self.functions[checked.function].modifies.iter().find(|global| !synced(global))
            {
                return Some((global, Some(current)));
            }
            queue.extend(checked.calls.iter().copied());
        }
        None
    }

    pub(crate) fn instance_display_name(&self, instance: usize) -> String {
        self.instance_name(instance)
    }

    /// The name of an instance's function, for messages.
    fn instance_name(&self, instance: usize) -> String {
        match &self.instances[instance].synthetic {
            Some((name, _)) => name.clone(),
            None => self.functions[self.instances[instance].function].full_name(),
        }
    }

    /// A function made by the checker, whose body is given by `finish_synthetic`.
    pub(crate) fn synthetic_instance(&mut self, name: String, span: Span, ret: Type) -> usize {
        self.instances.push(Instance {
            function: usize::MAX,
            synthetic: Some((name, span)),
            arg_types: Vec::new(),
            origin: None,
            ret: Ret::Declared(ret),
            bindings: HashMap::new(),
            state: BodyState::Checked,
            reads: Vec::new(),
            calls: Vec::new(),
            checked: None,
        });
        self.instances.len() - 1
    }

    pub(crate) fn finish_synthetic(
        &mut self,
        instance: usize,
        params: Vec<Type>,
        locals: Vec<LocalInfo>,
        body: Vec<ir::Stmt>,
        calls: Vec<usize>,
    ) {
        let instance = &mut self.instances[instance];
        instance.calls = calls;
        instance.checked = Some(CheckedBody { params, captures: 0, locals, body, defaults: Vec::new() });
    }

    /// A call of a function made by the checker, checked like the others when the
    /// script makes it (C3).
    pub(crate) fn calls_synthetic(&mut self, instance: usize, span: Span) {
        self.ctx.calls.push(instance);
        self.record_early_call(instance, span);
    }

    /// A call made by the script or by the values of the globals of a module: the
    /// function must not read a global that may have no value yet (C3).
    pub(crate) fn record_early_call(&mut self, instance: usize, span: Span) {
        let unassigned = match self.ctx.kind {
            ContextKind::Script => self.unassigned_globals(),
            // The globals of the module that come later have no value yet.
            ContextKind::Init(_) => self
                .tables
                .globals
                .values()
                .filter(|global| matches!(global.ty, GlobalType::Unknown))
                .filter_map(|global| global.local)
                .collect(),
            _ => return,
        };
        self.script_calls.push(ScriptCall { instance, span, unassigned });
    }

    /// The globals that may have no value at the current point of the script.
    fn unassigned_globals(&self) -> Vec<ir::LocalId> {
        self.tables
            .globals
            .values()
            .filter_map(|global| global.local)
            .filter(|&local| self.ctx.flow.get(local) != Assigned::Yes)
            .collect()
    }

    /// Whether the current function may assign the global `local` (§11.5).
    pub(crate) fn check_global_assignment(&mut self, local: ir::LocalId, span: Span) -> bool {
        let ContextKind::Function(instance) = self.ctx.kind else { return true };
        let function = &self.functions[self.instances[instance].function];
        let global = &self.global_info(local);
        let (decl, name) = (global.decl, &global.decl.name.name);
        let error = if !decl.mutable && decl.value.is_none() {
            Diagnostic::error(format!(
                "the constant `{name}` receives its value in the script, not in a function"
            ))
            .with_primary(span, "")
            .with_secondary(decl.name.span, "declared here without a value")
        } else if !decl.mutable {
            Diagnostic::error(format!("cannot assign to the constant `{name}`"))
                .with_primary(span, "")
                .with_secondary(decl.name.span, "declared with `let`")
        } else if !function.modifies.contains(&local) {
            Diagnostic::error(format!("`{}` modifies `{name}` without saying so", function.decl.name.name))
                .with_primary(span, "")
                .with_secondary(function.decl.name.span, format!("add `modifies {name}` to this declaration"))
                .with_note("a function announces the variables of the script it changes (§11.5)")
        } else {
            return true;
        };
        self.diagnostics.push(error);
        false
    }

    /// Each call made by the script: the function must not read, directly or through
    /// the functions it calls, a global that may have no value there (C3).
    pub(crate) fn check_script_calls(&mut self) {
        let mut reads: Vec<HashSet<ir::LocalId>> =
            self.instances.iter().map(|instance| instance.reads.iter().copied().collect()).collect();
        loop {
            let mut changed = false;
            for caller in 0..self.instances.len() {
                for &callee in &self.instances[caller].calls {
                    let missing: Vec<ir::LocalId> =
                        reads[callee].difference(&reads[caller]).copied().collect();
                    if !missing.is_empty() {
                        reads[caller].extend(missing);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for call in std::mem::take(&mut self.script_calls) {
            let mut unassigned: Vec<ir::LocalId> = call
                .unassigned
                .iter()
                .copied()
                .filter(|global| reads[call.instance].contains(global))
                .collect();
            unassigned.sort_by_key(|local| local.0);
            let test = self
                .instance_function(call.instance)
                .and_then(|function| self.functions[function].test.clone());
            for global in unassigned {
                let decl = self.global_info(global).decl;
                if let Some(test) = &test {
                    self.diagnostics.push(
                        Diagnostic::error(format!("the test \"{test}\" reads `{}`, a variable of the script", decl.name.name))
                            .with_primary(call.span, "")
                            .with_secondary(decl.name.span, "declared here")
                            .with_note("before a test, `lion test` gives values only to the variables declared with one; the other statements of the script do not run (C68)")
                            .with_help("give the variable its value where it is declared, or give the test its own variables"),
                    );
                    continue;
                }
                let function = &self.instance_name(call.instance);
                self.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{function}` may read `{}`, which may have no value at this call",
                        decl.name.name
                    ))
                    .with_primary(call.span, "")
                    .with_secondary(decl.name.span, "declared here")
                    .with_note("a function may read a variable declared further down, but only once it has a value (§6.1, §6.5)"),
                );
            }
        }
    }
}

/// `l[i]`, `s.grade`: a part of a variable, which a `var` parameter can change.
fn is_part_of_variable(expr: &ast::Expr) -> bool {
    match &expr.kind {
        ast::ExprKind::Index { object, .. } | ast::ExprKind::Field { object, .. } => {
            matches!(object.kind, ast::ExprKind::Name(_)) || is_part_of_variable(object)
        }
        ast::ExprKind::Paren(inner) => is_part_of_variable(inner),
        _ => false,
    }
}

/// Converts Int return values to Float, once the return type is known to be Float.
fn widen_returns(stmts: &mut [ir::Stmt]) {
    for stmt in stmts {
        match stmt {
            ir::Stmt::Return(Some(value)) if value.ty == Type::Int => {
                let span = value.span;
                let int = std::mem::replace(value, typed(ir::ExprKind::None, Type::None, span));
                *value = typed(
                    ir::ExprKind::Convert { conversion: ir::Conversion::IntToFloat, value: Box::new(int) },
                    Type::Float,
                    span,
                );
            }
            ir::Stmt::If { then, otherwise, .. } => {
                widen_returns(then);
                widen_returns(otherwise);
            }
            ir::Stmt::While { body, .. } | ir::Stmt::For { body, .. } | ir::Stmt::Seq(body) => {
                widen_returns(body)
            }
            ir::Stmt::Parallel(parallel) => widen_returns(&mut parallel.body),
            ir::Stmt::Assign { .. }
            | ir::Stmt::InitModule { .. }
            | ir::Stmt::Declare { .. }
            | ir::Stmt::Expr(_)
            | ir::Stmt::AssignElement { .. }
            | ir::Stmt::Add { .. }
            | ir::Stmt::Remove { .. }
            | ir::Stmt::Break
            | ir::Stmt::Continue
            | ir::Stmt::Return(_) => {}
        }
    }
}
