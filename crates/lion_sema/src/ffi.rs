//! Calling C (spec §21.2): `foreign "libm" pure fun cos(x in Float) in Float` declares a
//! C function and its library; it is called only in an `unsafe` block, since the
//! compiler of Lion can check nothing in it (C80).

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Type};
use lion_syntax::ast;

use crate::{Checker, article};

/// At most this many arguments of each kind, as the registers of the machine hold them.
const MAX_INTEGERS: usize = 6;
const MAX_FLOATS: usize = 8;

impl Checker<'_> {
    /// A C function: the types of its parameters and of its value have a C form: Int
    /// (`int64_t`), Float (`double`), Bool (`int`), Text (`const char *`), and for the
    /// value also None (`void`) and `maybe Text` (a null pointer is `none`).
    pub(crate) fn c_function(&mut self, index: usize, foreign: &ast::Foreign) -> bool {
        let decl = self.functions[index].decl.clone();
        let mut valid = true;
        if foreign.library.is_empty() {
            {
                self.ffi_error(
                    foreign.span,
                    "a foreign function names its library: `foreign \"libm\"`".to_string(),
                    "(§21.2)",
                );
                valid = false;
            }
        }
        if let Some(receiver) = &decl.receiver {
            {
                self.ffi_error(
                    receiver.span,
                    "a C function is not a method".to_string(),
                    "declare it `foreign \"libm\" fun name(...)` (§21.2)",
                );
                valid = false;
            }
        }
        let (mut integers, mut floats) = (0, 0);
        let mut params = Vec::new();
        for param in &decl.params {
            if let Some(span) = param.var {
                {
                    self.ffi_error(
                        span,
                        "a C function has no `var` parameters".to_string(),
                        "the values are given to C, which returns one value (C80)",
                    );
                    valid = false;
                }
            }
            if let Some(default) = &param.default {
                {
                    self.ffi_error(default.span, "a C function has no default values".to_string(), "(C80)");
                    valid = false;
                }
            }
            let Some(ty) = &param.ty else {
                {
                    self.ffi_error(
                        param.name.span,
                        format!("the type of `{}` must be written", param.name.name),
                        "a foreign function writes the types of its parameters (§21.2)",
                    );
                    valid = false;
                }
                continue;
            };
            let Some(ty) = self.resolve_type(ty) else {
                valid = false;
                continue;
            };
            match ty {
                Type::Float => floats += 1,
                Type::Int | Type::Bool | Type::Text => integers += 1,
                other => {
                    {
                        self.ffi_error(
                            param.name.span,
                            format!("a C function does not take {}", article(other)),
                            "its parameters are Int, Float, Bool or Text values (C80)",
                        );
                        valid = false;
                    }
                    continue;
                }
            }
            params.push(ty);
        }
        if integers > MAX_INTEGERS || floats > MAX_FLOATS {
            {
                self.ffi_error(decl.name.span, format!("a C function takes at most {MAX_INTEGERS} Int, Bool or Text values and {MAX_FLOATS} Float values"), "(C80)");
                valid = false;
            }
        }
        let ret = match &decl.ret {
            None => Type::None,
            Some(ty) => match self.resolve_type(ty) {
                Some(ty) => ty,
                None => return false,
            },
        };
        let allowed = matches!(ret, Type::Int | Type::Float | Type::Bool | Type::Text | Type::None)
            || ret == Type::maybe(Type::Text);
        if !allowed {
            let span = decl.ret.as_ref().map_or(decl.name.span, |ty| ty.span);
            {
                self.ffi_error(
                    span,
                    format!("a C function does not give {}", article(ret)),
                    "it gives an Int, a Float, a Bool, a Text, `maybe Text` or nothing (C80)",
                );
                valid = false;
            }
        }
        if !valid {
            return false;
        }
        let position = self.foreign.len() as u32;
        self.foreign.push(ir::ForeignFunction {
            library: foreign.library.clone(),
            name: decl.name.name.clone(),
            params,
            ret,
        });
        self.functions[index].foreign = Some((position, foreign.pure));
        true
    }

    fn ffi_error(&mut self, span: Span, message: String, note: &str) {
        self.diagnostics.push(Diagnostic::error(message).with_primary(span, "").with_note(note.to_string()));
    }

    /// A C function is called only in an `unsafe` block (§21.2).
    pub(crate) fn check_c_call(&mut self, index: usize, span: Span) -> bool {
        if self.functions[index].foreign.is_none() || self.ctx.unsafe_depth > 0 {
            return true;
        }
        let name = self.functions[index].decl.name.name.clone();
        self.diagnostics.push(
            Diagnostic::error(format!("`{name}` is a C function: it is called in an `unsafe` block"))
                .with_primary(span, "")
                .with_note("the compiler of Lion can check nothing in C code (§21.2)")
                .with_help("wrap the call: `unsafe:` on its own line, the call, then `;`, in a function that gives a safe value"),
        );
        false
    }

    /// `unsafe: ... ;`: its statements may call C functions (§21.2).
    pub(crate) fn unsafe_block(&mut self, body: &ast::Block) -> Option<ir::Stmt> {
        self.ctx.unsafe_depth += 1;
        let stmts = self.block(body);
        self.ctx.unsafe_depth -= 1;
        Some(ir::Stmt::Seq(stmts))
    }
}
