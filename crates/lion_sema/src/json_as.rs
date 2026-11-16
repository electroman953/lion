//! `v as T`, with `v` a `json.Value` (C107, decision of the author): the value read as a
//! value of Lion of type `T`, or an Error that says where and why it does not fit.
//!
//! The checker makes one function per target type, `json.Value as T`, written in IR as
//! the constructor of a structure is (§12.3): it reads the kind of the value, goes into
//! the members and elements with the functions of the module `json`, and calls the
//! function of each inner type, itself included for a recursive structure. Both modes
//! run this IR, so they give the same values and the same Errors.

use lion_diagnostics::{Diagnostic, Span};
use lion_ir::{self as ir, Native, StructRef, Type};

use crate::{Checker, Context, ContextKind, article, typed};

impl Checker<'_> {
    /// The structure `json.Value`, when the program uses the module `json`.
    pub(crate) fn json_value_type(&self) -> Option<StructRef> {
        self.structs
            .iter()
            .find(|info| {
                info.decl.name.name == "Value"
                    && info.type_args.is_empty()
                    && self.modules[info.module].standard
                    && self.modules[info.module].name == "json"
            })
            .map(|info| info.id)
    }

    /// `value as target`, where `value` is a `json.Value`: a `target or Error`.
    pub(crate) fn json_as(&mut self, value: ir::Expr, target: Type, span: Span) -> Option<ir::Expr> {
        let json = self.json_value_type().expect("the value is a json.Value");
        if let Some(problem) = self.json_unsupported(target, json, &mut Vec::new()) {
            self.diagnostics.push(
                Diagnostic::error(format!("cannot read a json.Value as {}", article(target)))
                    .with_primary(span, problem)
                    .with_note("a json.Value reads as Text, Int, Float, Bool, `none`, a List, a Map of (Text, …), a structure, an enumeration, or a union of them (C107)"),
            );
            return None;
        }
        let converter = self.json_converter(target, json, span);
        self.calls_synthetic(converter, span);
        let call = ir::ExprKind::Call {
            function: ir::FunctionId(converter as u32),
            args: vec![ir::Arg::Value(value)],
        };
        Some(typed(call, Type::union([target, Type::Error]), span))
    }

    /// Why a json.Value cannot be read as `target`, if it cannot.
    fn json_unsupported(&mut self, target: Type, json: StructRef, seen: &mut Vec<Type>) -> Option<String> {
        if seen.contains(&target) {
            return None;
        }
        seen.push(target);
        match target {
            Type::Text | Type::Int | Type::Float | Type::Bool | Type::None | Type::Error | Type::Enum(_) => {
                None
            }
            Type::Struct(id) if id == json => None,
            Type::List(element) => self.json_unsupported(element.get(), json, seen),
            Type::Map(_) => {
                let (key, value) = target.map_parts().expect("a Map");
                if key != Type::Text {
                    return Some(format!("the keys of a JSON object are Texts, not {}", article(key)));
                }
                self.json_unsupported(value, json, seen)
            }
            Type::Union(_) => {
                target.members().into_iter().find_map(|member| self.json_unsupported(member, json, seen))
            }
            Type::Struct(id) => {
                let index = self.struct_index(id);
                let fields = self.structs[index].fields.clone();
                for (position, field) in fields.iter().enumerate() {
                    if self.field_is_private(index, position) && self.structs[index].module != self.module {
                        return Some(format!("the field `{}` of {target} is private (D10)", field.name));
                    }
                    let Some(ty) = field.ty else {
                        return Some(format!("the field `{}` has no known type", field.name));
                    };
                    if let Some(problem) = self.json_unsupported(ty, json, seen) {
                        return Some(format!("the field `{}`: {problem}", field.name));
                    }
                }
                None
            }
            other => Some(format!("JSON has no form for {}", article(other))),
        }
    }

    /// The function `json.Value as target`, made once per type.
    fn json_converter(&mut self, target: Type, json: StructRef, span: Span) -> usize {
        if let Some(&made) = self.json_converters.get(&target) {
            return made;
        }
        let result = Type::union([target, Type::Error]);
        let instance = self.synthetic_instance(format!("json.Value as {target}"), span, result);
        self.json_converters.insert(target, instance);
        let outer = std::mem::replace(&mut self.ctx, Context::new(ContextKind::Script));
        let body = self.json_body(target, json, span);
        let ctx = std::mem::replace(&mut self.ctx, outer);
        self.finish_synthetic(instance, vec![Type::Struct(json)], ctx.locals, body, ctx.calls);
        instance
    }

    /// The statements of `json.Value as target`; its parameter is the first local.
    fn json_body(&mut self, target: Type, json: StructRef, span: Span) -> Vec<ir::Stmt> {
        let b = Build { span, json };
        let value = self.json_local("v", Type::Struct(json), span);
        let source = self.temporary(Type::Text, span);
        let kind = self.temporary(Type::Text, span);
        let mut body = vec![
            ir::Stmt::Assign {
                place: ir::Place::Local(source),
                value: typed(
                    ir::ExprKind::Field { object: Box::new(b.local(value, Type::Struct(json))), field: 0 },
                    Type::Text,
                    span,
                ),
            },
            ir::Stmt::Assign {
                place: ir::Place::Local(kind),
                value: b.native(Native::JsonKind, vec![b.local(source, Type::Text)], Type::Text),
            },
        ];
        let src = || b.local(source, Type::Text);
        let scalar = || b.native(Native::JsonScalar, vec![b.local(source, Type::Text)], Type::Text);
        // `if kind != wanted: return error("the value is a {kind}, not …")`.
        let expect = |wanted: &str, body: &mut Vec<ir::Stmt>| {
            body.push(ir::Stmt::If {
                cond: b.not(b.eq(b.local(kind, Type::Text), b.text(wanted))),
                then: vec![b.ret(b.mismatch(kind, target))],
                otherwise: Vec::new(),
            });
        };
        match target {
            Type::Struct(id) if id == json => body.push(b.ret(b.local(value, Type::Struct(json)))),
            Type::Text => {
                expect("text", &mut body);
                body.push(b.ret(scalar()));
            }
            Type::Int | Type::Float => {
                expect("number", &mut body);
                let (conversion, ty) = if target == Type::Int {
                    (ir::Conversion::TextToInt, Type::Int)
                } else {
                    (ir::Conversion::TextToFloat, Type::Float)
                };
                let converted = typed(
                    ir::ExprKind::Convert { conversion, value: Box::new(scalar()) },
                    Type::union([ty, Type::Error]),
                    span,
                );
                body.push(b.ret(converted));
            }
            Type::Bool => {
                expect("bool", &mut body);
                body.push(b.ret(b.eq(scalar(), b.text("true"))));
            }
            Type::None => {
                expect("null", &mut body);
                body.push(b.ret(typed(ir::ExprKind::None, Type::None, span)));
            }
            Type::Error => body.push(b.ret(b.mismatch(kind, target))),
            Type::Enum(enumeration) => {
                expect("text", &mut body);
                for (position, name) in enumeration.values().iter().enumerate() {
                    body.push(ir::Stmt::If {
                        cond: b.eq(scalar(), b.text(name)),
                        then: vec![b.ret(typed(
                            ir::ExprKind::Enum { enumeration, value: position as u32 },
                            target,
                            span,
                        ))],
                        otherwise: Vec::new(),
                    });
                }
                let message =
                    b.concat(vec![b.text("`"), scalar(), b.text(format!("` is not a value of {target}"))]);
                body.push(b.ret(b.error(message)));
            }
            Type::List(element) => {
                expect("list", &mut body);
                let element = element.get();
                let out = self.json_local("%list", target, span);
                let position = self.json_local("%i", Type::Int, span);
                let part = self.json_local("%part", Type::Text, span);
                let item = self.temporary(Type::union([element, Type::Error]), span);
                let converter = self.json_converter(element, json, span);
                self.calls_synthetic(converter, span);
                body.push(ir::Stmt::Assign {
                    place: ir::Place::Local(out),
                    value: typed(ir::ExprKind::List(Vec::new()), target, span),
                });
                body.push(ir::Stmt::Assign {
                    place: ir::Place::Local(position),
                    value: typed(ir::ExprKind::Int(0), Type::Int, span),
                });
                let step = b.concat(vec![b.text("["), b.to_text(b.local(position, Type::Int)), b.text("]")]);
                let turn = vec![
                    ir::Stmt::Assign {
                        place: ir::Place::Local(position),
                        value: b.binary(
                            ir::BinaryOp::AddInt,
                            b.local(position, Type::Int),
                            typed(ir::ExprKind::Int(1), Type::Int, span),
                            Type::Int,
                        ),
                    },
                    ir::Stmt::Assign {
                        place: ir::Place::Local(item),
                        value: b.convert_part(converter, b.local(part, Type::Text), element),
                    },
                    b.fail_with(item, element, step),
                    ir::Stmt::Add {
                        root: ir::Place::Local(out),
                        path: Vec::new(),
                        value: b.local(item, element),
                    },
                ];
                body.push(ir::Stmt::For {
                    var: part,
                    iterable: b.native(Native::JsonParts, vec![src()], Type::list(Type::Text)),
                    body: turn,
                });
                body.push(b.ret(b.local(out, target)));
            }
            Type::Map(_) => {
                expect("object", &mut body);
                let (_, element) = target.map_parts().expect("a Map");
                let out = self.json_local("%map", target, span);
                let name = self.json_local("%name", Type::Text, span);
                let member = self.temporary(Type::union([Type::Text, Type::Error]), span);
                let item = self.temporary(Type::union([element, Type::Error]), span);
                let converter = self.json_converter(element, json, span);
                self.calls_synthetic(converter, span);
                body.push(ir::Stmt::Assign {
                    place: ir::Place::Local(out),
                    value: typed(ir::ExprKind::Map(Vec::new()), target, span),
                });
                let turn = vec![
                    ir::Stmt::Assign {
                        place: ir::Place::Local(member),
                        value: b.native(
                            Native::JsonMember,
                            vec![src(), b.local(name, Type::Text)],
                            Type::union([Type::Text, Type::Error]),
                        ),
                    },
                    ir::Stmt::Assign {
                        place: ir::Place::Local(item),
                        value: b.convert_part(converter, b.local(member, Type::Text), element),
                    },
                    b.fail_with(item, element, b.local(name, Type::Text)),
                    ir::Stmt::AssignElement {
                        root: ir::Place::Local(out),
                        path: vec![ir::Step::Index(b.local(name, Type::Text))],
                        value: b.local(item, element),
                    },
                ];
                body.push(ir::Stmt::For {
                    var: name,
                    iterable: b.native(Native::JsonNames, vec![src()], Type::list(Type::Text)),
                    body: turn,
                });
                body.push(b.ret(b.local(out, target)));
            }
            Type::Union(_) => {
                // The first member that reads the value; `maybe T` tries `none` too.
                for member in target.members() {
                    if member == Type::Error {
                        continue;
                    }
                    let converter = self.json_converter(member, json, span);
                    self.calls_synthetic(converter, span);
                    let tried = self.temporary(Type::union([member, Type::Error]), span);
                    let call = ir::ExprKind::Call {
                        function: ir::FunctionId(converter as u32),
                        args: vec![ir::Arg::Value(b.local(value, Type::Struct(json)))],
                    };
                    body.push(ir::Stmt::Assign {
                        place: ir::Place::Local(tried),
                        value: typed(call, Type::union([member, Type::Error]), span),
                    });
                    body.push(ir::Stmt::If {
                        cond: b.not(b.is_error(b.local(tried, Type::union([member, Type::Error])))),
                        then: vec![b.ret(b.local(tried, member))],
                        otherwise: Vec::new(),
                    });
                }
                body.push(b.ret(b.mismatch(kind, target)));
            }
            Type::Struct(id) => {
                expect("object", &mut body);
                let index = self.struct_index(id);
                let fields = self.structs[index].fields.clone();
                let mut values = Vec::new();
                for (position, field) in fields.iter().enumerate() {
                    let ty = field.ty.expect("checked by json_unsupported");
                    let member = self.temporary(Type::union([Type::Text, Type::Error]), span);
                    let found = self.json_local(&format!("%{}", field.name), ty, span);
                    body.push(ir::Stmt::Assign {
                        place: ir::Place::Local(member),
                        value: b.native(
                            Native::JsonMember,
                            vec![src(), b.text(&field.name)],
                            Type::union([Type::Text, Type::Error]),
                        ),
                    });
                    // A missing member takes the default value of the field, or `none`
                    // for a `maybe` field; otherwise the Error says it is missing.
                    let missing = match self.field_default(index, position, span) {
                        Some(default) => ir::Stmt::Assign { place: ir::Place::Local(found), value: default },
                        None if Type::None.is_subset_of(ty) => ir::Stmt::Assign {
                            place: ir::Place::Local(found),
                            value: typed(ir::ExprKind::None, Type::None, span),
                        },
                        None => b.ret(b.error(b.text(format!("there is no member `{}`", field.name)))),
                    };
                    let converter = self.json_converter(ty, json, span);
                    self.calls_synthetic(converter, span);
                    let item = self.temporary(Type::union([ty, Type::Error]), span);
                    let present = vec![
                        ir::Stmt::Assign {
                            place: ir::Place::Local(item),
                            value: b.convert_part(converter, b.local(member, Type::Text), ty),
                        },
                        b.fail_with(item, ty, b.text(&field.name)),
                        ir::Stmt::Assign { place: ir::Place::Local(found), value: b.local(item, ty) },
                    ];
                    body.push(ir::Stmt::If {
                        cond: b.is_error(b.local(member, Type::union([Type::Text, Type::Error]))),
                        then: vec![missing],
                        otherwise: present,
                    });
                    values.push(b.local(found, ty));
                }
                let built = self.build_struct(index, values, span);
                body.push(b.ret(built));
            }
            _ => unreachable!("refused by json_unsupported"),
        }
        body
    }

    /// A variable of the function being made, which the program cannot name.
    fn json_local(&mut self, name: &str, ty: Type, span: Span) -> ir::LocalId {
        let id = self.temporary(ty, span);
        let local = &mut self.ctx.locals[id.index()];
        local.name = name.to_string();
        local.mutable = true;
        id
    }
}

/// Small pieces of IR.
struct Build {
    span: Span,
    json: StructRef,
}

impl Build {
    fn local(&self, local: ir::LocalId, ty: Type) -> ir::Expr {
        typed(ir::ExprKind::Local(local), ty, self.span)
    }

    fn text(&self, text: impl Into<String>) -> ir::Expr {
        typed(ir::ExprKind::Text(text.into()), Type::Text, self.span)
    }

    fn native(&self, native: Native, args: Vec<ir::Expr>, ty: Type) -> ir::Expr {
        typed(ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Native(native), args }, ty, self.span)
    }

    fn binary(&self, op: ir::BinaryOp, lhs: ir::Expr, rhs: ir::Expr, ty: Type) -> ir::Expr {
        typed(ir::ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, ty, self.span)
    }

    fn eq(&self, lhs: ir::Expr, rhs: ir::Expr) -> ir::Expr {
        self.binary(ir::BinaryOp::EqText, lhs, rhs, Type::Bool)
    }

    fn not(&self, operand: ir::Expr) -> ir::Expr {
        typed(ir::ExprKind::Unary { op: ir::UnaryOp::Not, operand: Box::new(operand) }, Type::Bool, self.span)
    }

    fn concat(&self, parts: Vec<ir::Expr>) -> ir::Expr {
        typed(ir::ExprKind::Concat(parts), Type::Text, self.span)
    }

    fn to_text(&self, value: ir::Expr) -> ir::Expr {
        typed(
            ir::ExprKind::Convert { conversion: ir::Conversion::ToText, value: Box::new(value) },
            Type::Text,
            self.span,
        )
    }

    fn error(&self, message: ir::Expr) -> ir::Expr {
        typed(
            ir::ExprKind::CallBuiltin { builtin: ir::Builtin::Error, args: vec![message] },
            Type::Error,
            self.span,
        )
    }

    fn is_error(&self, value: ir::Expr) -> ir::Expr {
        typed(ir::ExprKind::TypeTest { value: Box::new(value), ty: Type::Error }, Type::Bool, self.span)
    }

    fn ret(&self, value: ir::Expr) -> ir::Stmt {
        ir::Stmt::Return(Some(value))
    }

    /// `error("the value is a {kind}, not a T")`.
    fn mismatch(&self, kind: ir::LocalId, target: Type) -> ir::Expr {
        let expected = match target {
            Type::Text => "a text".to_string(),
            Type::Int => "an Int".to_string(),
            Type::Float => "a number".to_string(),
            Type::Bool => "a Bool".to_string(),
            Type::None => "null".to_string(),
            Type::List(_) => "a list".to_string(),
            Type::Map(_) | Type::Struct(_) => "an object".to_string(),
            other => article(other),
        };
        self.error(self.concat(vec![
            self.text("the value is a "),
            self.local(kind, Type::Text),
            self.text(format!(", not {expected}")),
        ]))
    }

    /// The function `converter` called on the JSON text `part`.
    fn convert_part(&self, converter: usize, part: ir::Expr, ty: Type) -> ir::Expr {
        let value = typed(
            ir::ExprKind::Struct { structure: self.json, fields: vec![part] },
            Type::Struct(self.json),
            self.span,
        );
        let call = ir::ExprKind::Call {
            function: ir::FunctionId(converter as u32),
            args: vec![ir::Arg::Value(value)],
        };
        typed(call, Type::union([ty, Type::Error]), self.span)
    }

    /// `if item in Error: return error(json.nest(step, item.message()))`: the Error of an
    /// inner value, with where it was found.
    fn fail_with(&self, item: ir::LocalId, ty: Type, step: ir::Expr) -> ir::Stmt {
        let failed = self.local(item, Type::union([ty, Type::Error]));
        let message = typed(
            ir::ExprKind::CallBuiltin {
                builtin: ir::Builtin::Message,
                args: vec![self.local(item, Type::Error)],
            },
            Type::Text,
            self.span,
        );
        let nested = self.native(Native::JsonNest, vec![step, message], Type::Text);
        ir::Stmt::If {
            cond: self.is_error(failed),
            then: vec![self.ret(self.error(nested))],
            otherwise: Vec::new(),
        }
    }
}
