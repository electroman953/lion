//! Calls of methods (spec §12.4): `object.name(args)`, where `object` becomes `self`.
//!
//! A method declared with `var self` changes its object: the object is then a place,
//! a `var` variable or a part of one, like the argument of a `var` parameter (§11.2).
//! Once the outermost call on an object returns, its invariants are checked (§12.3,
//! D9): inside a `var self` method, a call on `self` itself is not checked.

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::Type;
use lion_syntax::ast;

use crate::functions::Pending;
use crate::{Checker, GlobalType, article, capitalize};

impl Checker<'_> {
    /// `object.name(args)` (§12.4).
    pub(crate) fn method_call(
        &mut self,
        object: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<lion_ir::Expr> {
        if let Some(ty) = self.place_type(object)
            && let Some(method) = self.visible_method(ty, &name.name)
            && self.functions[method].var_self
        {
            let reported = self.diagnostics.len();
            let Some(path) = self.place_path(object) else {
                // Say why the object must be changeable.
                if let Some(error) = self.diagnostics.get_mut(reported) {
                    error.notes.insert(
                        0,
                        format!("`{}` changes its object: it is declared with `var self` (§12.4)", name.name),
                    );
                }
                return None;
            };
            let (setup, path) = self.stabilize(path);
            let after = self.check_invariants(&path, true);
            let receiver = if path.steps.is_empty() {
                Pending::Place(path.root, path.ty())
            } else {
                Pending::Element(path, setup)
            };
            return self.call_with(method, Some(receiver), after, name.span, args, span);
        }
        let value = self.expr(object)?;
        let value = self.within_try(value);
        if let Some((key, entry)) = value.ty.map_parts() {
            return self.map_method(value, key, entry, name, args, span);
        }
        // A value of several possible types: the method of each, chosen at run time (§14.4).
        let members = value.ty.members();
        if self.visible_method(value.ty, &name.name).is_none()
            && (members.len() > 1 || members != [value.ty])
            && members.iter().all(|&member| self.visible_method(member, &name.name).is_some())
        {
            return self.dispatch_call(value, name, args, span);
        }
        let Some(method) = self.visible_method(value.ty, &name.name) else {
            // A field that holds a function: `button.on_click()` (§11).
            if let Type::Struct(structure) = value.ty {
                let info = &self.structs[self.struct_index(structure)];
                let field = info.fields.iter().position(|field| field.name == name.name);
                if let Some(field) =
                    field.filter(|&field| matches!(info.fields[field].ty, Some(Type::Fun(_))))
                {
                    let ty = info.fields[field].ty.expect("checked");
                    let span = value.span.to(name.span);
                    let kind = lion_ir::ExprKind::Field { object: Box::new(value), field: field as u32 };
                    return self.call_value(crate::typed(kind, ty, span), args, span);
                }
            }
            self.no_method(value.ty, name, value.span);
            return None;
        };
        // A method that changes a temporary value: the change is lost (§11.2).
        let receiver =
            if self.functions[method].var_self { Pending::Temporary(value) } else { Pending::Value(value) };
        self.call_with(method, Some(receiver), Vec::new(), name.span, args, span)
    }

    /// `m.get(k)`: the value of the key, or `none` (C79).
    fn map_method(
        &mut self,
        map: lion_ir::Expr,
        key: Type,
        entry: Type,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<lion_ir::Expr> {
        match (name.name.as_str(), args) {
            ("get", [arg]) if arg.name.is_none() && arg.var_marker.is_none() => {
                let given = self.expr(&arg.value)?;
                let context = (map.span, format!("the keys of this Map are {}", article(key)));
                let given = self.coerce(given, key, Some(context))?;
                let kind = lion_ir::ExprKind::CallBuiltin {
                    builtin: lion_ir::Builtin::MapGet,
                    args: vec![map, given],
                };
                Some(crate::typed(kind, Type::maybe(entry), span))
            }
            ("remove", _) => {
                self.diagnostics.push(
                    Diagnostic::error("`remove` is called on its own line: `m.remove(key)`")
                        .with_primary(span, "")
                        .with_note("`remove` changes the Map and gives no value (C79)"),
                );
                None
            }
            ("get", _) => {
                self.diagnostics
                    .push(Diagnostic::error("`get` takes one key: `m.get(key)`").with_primary(span, ""));
                None
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error(format!("a Map has no method `{}`", name.name))
                        .with_primary(name.span, "")
                        .with_note("a Map has `m[key]`, `m.get(key)`, `m.remove(key)`, `key in m` and `m.size` (C79)"),
                );
                None
            }
        }
    }

    fn no_method(&mut self, ty: Type, name: &ast::Ident, object: Span) {
        let mut error =
            Diagnostic::error(format!("{} has no method `{}`", capitalize(&article(ty)), name.name))
                .with_primary(name.span, "")
                .with_secondary(object, format!("this is {}", article(ty)));
        let members = ty.members();
        if members.len() > 1
            && members.iter().any(|member| self.visible_method(*member, &name.name).is_some())
            && let Some(help) = crate::expr::union_help(ty)
        {
            error = error.with_help(help);
        } else if let Type::Struct(structure) = ty
            && self.field_names(structure).contains(&name.name)
        {
            error =
                error.with_note(format!("`{0}` is a field: write `.{0}`, without parentheses", name.name));
        } else {
            error = error.with_note(format!("a method is declared `fun {ty}.{}(...)` (§12.4)", name.name));
        }
        self.diagnostics.push(error);
    }

    /// Whether `object` is a variable, or a part of one, whose type has a method `add`:
    /// then `object.add(...)` calls it, rather than adding to a List.
    pub(crate) fn has_own_add(&self, object: &ast::Expr) -> bool {
        self.place_type(object).is_some_and(|ty| self.visible_method(ty, "add").is_some())
    }

    /// The method `name` of the type `ty` that the module being checked sees: one of
    /// its own, or of a module it uses (§20.3).
    pub(crate) fn visible_method(&self, ty: Type, name: &str) -> Option<usize> {
        let methods = self.methods.get(&(ty, name.to_string()))?;
        methods.iter().copied().find(|&method| self.sees(self.functions[method].module))
    }

    /// The type of `expr` when it is a variable or a part of one, found without checking
    /// anything or reporting; `None` otherwise.
    pub(crate) fn place_type(&self, expr: &ast::Expr) -> Option<Type> {
        match &expr.kind {
            ast::ExprKind::Name(name) => match self.lookup(name) {
                Some(local) => self.local_type(local),
                None if self.ctx.kind != crate::ContextKind::Script => {
                    match self.tables.globals.get(name)?.ty {
                        GlobalType::Known(ty) => ty,
                        GlobalType::Unknown => None,
                    }
                }
                None => None,
            },
            ast::ExprKind::Paren(inner) => self.place_type(inner),
            ast::ExprKind::Index { object, .. } => match self.place_type(object)? {
                Type::List(element) => Some(element.get()),
                map @ Type::Map(_) => map.map_parts().map(|(_, value)| value),
                _ => None,
            },
            ast::ExprKind::Field { object, name } => match self.place_type(object)? {
                Type::Struct(structure) => {
                    let info = &self.structs[self.struct_index(structure)];
                    info.fields.iter().find(|field| field.name == name.name)?.ty
                }
                _ => None,
            },
            _ => None,
        }
    }
}
