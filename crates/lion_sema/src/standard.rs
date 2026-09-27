//! The functions of the core of the standard library, available without `use` (spec
//! §23): `ask`, `exit`, `reverse`, `floor`, `ceil`, `round`, `isqrt`. `show`, `sum`
//! and `error` are checked with the expressions. Their parameters have no names (R7).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::{Checker, article, typed};

impl Checker<'_> {
    pub(crate) fn standard_call(&mut self, name: &str, args: &[ast::Arg], span: Span) -> Option<ir::Expr> {
        let (min, max) = match name {
            "ask" | "exit" => (0, 1),
            _ => (1, 1),
        };
        let mut valid = true;
        for arg in args {
            if let Some(var) = arg.var_marker {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{name}` does not modify its argument"))
                        .with_primary(var, "remove this `var`"),
                );
                valid = false;
            }
            if let Some(arg_name) = &arg.name {
                self.diagnostics.push(
                    Diagnostic::error(format!("`{name}` does not take named arguments"))
                        .with_primary(arg_name.span, "")
                        .with_note("Lion 0.1 does not name the parameters of the standard functions"),
                );
                valid = false;
            }
        }
        if args.len() < min || args.len() > max {
            let expected = if min == max { format!("{min}") } else { format!("{min} or {max}") };
            let plural = if max == 1 { "" } else { "s" };
            self.diagnostics.push(
                Diagnostic::error(format!("`{name}` takes {expected} argument{plural}, not {}", args.len()))
                    .with_primary(span, ""),
            );
            valid = false;
        }
        let values: Vec<Option<ir::Expr>> = args.iter().map(|arg| self.expr(&arg.value)).collect();
        if !valid {
            return None;
        }
        let values: Vec<ir::Expr> = values.into_iter().collect::<Option<_>>()?;
        let values: Vec<ir::Expr> = values.into_iter().map(|value| self.within_try(value)).collect();
        let call = |builtin, args, ty| typed(ir::ExprKind::CallBuiltin { builtin, args }, ty, span);
        let mut values = values.into_iter();
        match name {
            "ask" => {
                let prompt = match values.next() {
                    Some(prompt) => self.expect_argument(name, prompt, Type::Text)?,
                    None => typed(ir::ExprKind::Text(String::new()), Type::Text, span),
                };
                Some(call(ir::Builtin::Ask, vec![prompt], Type::Text))
            }
            "exit" => {
                let code = match values.next() {
                    Some(code) => self.expect_argument(name, code, Type::Int)?,
                    None => typed(ir::ExprKind::Int(0), Type::Int, span),
                };
                Some(call(ir::Builtin::Exit, vec![code], Type::None))
            }
            "isqrt" => {
                let value = self.expect_argument(name, values.next()?, Type::Int)?;
                Some(call(ir::Builtin::Isqrt, vec![value], Type::Int))
            }
            "floor" | "ceil" | "round" => {
                let value = values.next()?;
                match value.ty {
                    // An Int is already whole.
                    Type::Int => Some(ir::Expr { span, ..value }),
                    Type::Float => {
                        let builtin = match name {
                            "floor" => ir::Builtin::Floor,
                            "ceil" => ir::Builtin::Ceil,
                            _ => ir::Builtin::Round,
                        };
                        Some(call(builtin, vec![value], Type::Int))
                    }
                    other => {
                        self.argument_error(name, &value, other, "a number");
                        None
                    }
                }
            }
            "reverse" => {
                let value = values.next()?;
                match value.ty {
                    Type::List(_) | Type::Text => {
                        let ty = value.ty;
                        Some(call(ir::Builtin::Reverse, vec![value], ty))
                    }
                    other => {
                        self.argument_error(name, &value, other, "a List or a Text");
                        None
                    }
                }
            }
            _ => unreachable!("not a function of the core"),
        }
    }

    fn expect_argument(&mut self, name: &str, value: ir::Expr, expected: Type) -> Option<ir::Expr> {
        if value.ty == expected {
            return Some(value);
        }
        let ty = value.ty;
        self.argument_error(name, &value, ty, &article(expected));
        None
    }

    fn argument_error(&mut self, name: &str, value: &ir::Expr, ty: Type, expected: &str) {
        let mut error = Diagnostic::error(format!("`{name}` takes {expected}, not {}", article(ty)))
            .with_primary(value.span, format!("this is {}", article(ty)));
        if let Some(help) = crate::expr::union_help(ty) {
            error = error.with_help(help);
        }
        self.diagnostics.push(error);
    }
}
