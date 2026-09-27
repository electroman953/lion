//! The outline of a file: its declarations, read from its syntax tree, which exists
//! even when the file has errors.

use lion_diagnostics::{SourceFile, Span};
use lion_runtime::json::Json;
use lion_syntax::ast;

use super::convert;

/// The kinds of symbols of the protocol.
mod kind {
    pub const METHOD: i64 = 6;
    pub const FIELD: i64 = 8;
    pub const ENUM: i64 = 10;
    pub const INTERFACE: i64 = 11;
    pub const FUNCTION: i64 = 12;
    pub const VARIABLE: i64 = 13;
    pub const CONSTANT: i64 = 14;
    pub const ENUM_MEMBER: i64 = 22;
    pub const STRUCT: i64 = 23;
}

/// The declarations at the top level of a file, with those they hold.
pub fn document_symbols(file: &SourceFile, module: &ast::Module) -> Json {
    let outline = Outline { file };
    Json::List(module.stmts.iter().filter_map(|stmt| outline.statement(stmt)).collect())
}

struct Outline<'a> {
    file: &'a SourceFile,
}

impl Outline<'_> {
    fn statement(&self, stmt: &ast::Stmt) -> Option<Json> {
        Some(match &stmt.kind {
            ast::StmtKind::Let(decl) => {
                let kind = if decl.mutable { kind::VARIABLE } else { kind::CONSTANT };
                let detail = decl.annotation.as_ref().map(|ty| format!("in {}", self.text(ty.span)));
                self.symbol(&decl.name.name, detail, kind, stmt.span, decl.name.span, Vec::new())
            }
            ast::StmtKind::Fun(decl) => self.function(decl, stmt.span),
            ast::StmtKind::Test { name, decl } => {
                let label = format!("test \"{name}\"");
                self.symbol(&label, None, kind::FUNCTION, stmt.span, decl.name.span, Vec::new())
            }
            ast::StmtKind::Struct(decl) => {
                let fields = decl
                    .lines
                    .iter()
                    .filter_map(|line| match line {
                        ast::StructLine::Field(field) => Some(self.field(&field.name, &field.ty)),
                        ast::StructLine::Invariant(_) => None,
                    })
                    .collect();
                self.symbol(&decl.name.name, None, kind::STRUCT, stmt.span, decl.name.span, fields)
            }
            ast::StmtKind::TypeDef(def) => match &def.kind {
                ast::TypeDefKind::Enum { values, .. } => {
                    let values = values
                        .iter()
                        .map(|value| {
                            self.symbol(
                                &value.name,
                                None,
                                kind::ENUM_MEMBER,
                                value.span,
                                value.span,
                                Vec::new(),
                            )
                        })
                        .collect();
                    self.symbol(&def.name.name, None, kind::ENUM, stmt.span, def.name.span, values)
                }
                ast::TypeDefKind::Union(members) => {
                    let detail = Some(self.text(members.span).to_string());
                    self.symbol(&def.name.name, detail, kind::ENUM, stmt.span, def.name.span, Vec::new())
                }
            },
            ast::StmtKind::Trait(decl) => {
                let mut members: Vec<Json> =
                    decl.fields.iter().map(|(name, ty)| self.field(name, ty)).collect();
                members.extend(decl.methods.iter().map(|method| self.function(method, method.name.span)));
                self.symbol(&decl.name.name, None, kind::INTERFACE, stmt.span, decl.name.span, members)
            }
            _ => return None,
        })
    }

    /// A function, with its parameters and its type as detail: `(a in Int) in Int`.
    fn function(&self, decl: &ast::FunDecl, span: Span) -> Json {
        let params: Vec<String> = decl
            .params
            .iter()
            .map(|param| {
                let mut text = if param.var.is_some() { "var ".to_string() } else { String::new() };
                text.push_str(&param.name.name);
                if let Some(ty) = &param.ty {
                    text.push_str(" in ");
                    text.push_str(self.text(ty.span));
                }
                if let Some(default) = &param.default {
                    text.push_str(" = ");
                    text.push_str(self.text(default.span));
                }
                text
            })
            .collect();
        let mut detail = format!("({})", params.join(", "));
        if let Some(ret) = &decl.ret {
            detail.push_str(" in ");
            detail.push_str(self.text(ret.span));
        }
        let (name, kind) = match &decl.receiver {
            Some(receiver) => (format!("{}.{}", receiver.name, decl.name.name), kind::METHOD),
            None => (decl.name.name.clone(), kind::FUNCTION),
        };
        let span = span.to(decl.name.span);
        self.symbol(&name, Some(detail), kind, span, decl.name.span, Vec::new())
    }

    fn field(&self, name: &ast::Ident, ty: &ast::TypeExpr) -> Json {
        let detail = Some(format!("in {}", self.text(ty.span)));
        self.symbol(&name.name, detail, kind::FIELD, name.span.to(ty.span), name.span, Vec::new())
    }

    fn symbol(
        &self,
        name: &str,
        detail: Option<String>,
        kind: i64,
        span: Span,
        selection: Span,
        children: Vec<Json>,
    ) -> Json {
        let mut members = vec![("name".to_string(), Json::text(name))];
        if let Some(detail) = detail {
            members.push(("detail".to_string(), Json::text(detail)));
        }
        members.push(("kind".to_string(), Json::int(kind)));
        members.push(("range".to_string(), convert::range(self.file, span.to(selection))));
        members.push(("selectionRange".to_string(), convert::range(self.file, selection)));
        if !children.is_empty() {
            members.push(("children".to_string(), Json::List(children)));
        }
        Json::Object(members)
    }

    fn text(&self, span: Span) -> &str {
        self.file.text().get(span.start as usize..span.end as usize).unwrap_or("")
    }
}
