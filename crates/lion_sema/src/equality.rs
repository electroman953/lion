//! Equality defined by a structure (spec §12.5): `fun Student.equals(other in Student)
//! in Bool`. The machine then compares two values of the structure with it, wherever
//! values are compared: `==`, `x in l`, and the elements of a Set (C76).

use std::collections::HashSet;

use lion_diagnostics::Diagnostic;
use lion_ir::Type;

use crate::Checker;

impl Checker<'_> {
    /// Checks the `equals` methods once the signatures are known, and marks the
    /// structures that have one.
    pub(crate) fn register_equalities(&mut self) {
        let methods: Vec<usize> = (0..self.functions.len())
            .filter(|&index| {
                self.functions[index].receiver.is_some() && self.functions[index].decl.name.name == "equals"
            })
            .collect();
        for method in methods {
            self.register_equality(method);
        }
        self.equalities_registered = true;
    }

    /// The method `equals` becomes the equality of its structure, if it has the right
    /// signature and is declared in the file of the structure (C76).
    pub(crate) fn register_equality(&mut self, method: usize) {
        let function = &self.functions[method];
        let decl = std::rc::Rc::clone(&function.decl);
        let Some(Type::Struct(structure)) = function.receiver else {
            let ty = function.receiver.expect("a method");
            self.diagnostics.push(
                Diagnostic::error(format!("the equality of {ty} cannot be redefined"))
                    .with_primary(decl.name.span, "")
                    .with_note("a structure defines its equality with `equals` (§12.5, C76)"),
            );
            return;
        };
        let index = self.struct_index(structure);
        let ty = Type::Struct(structure);
        let fits = !function.var_self
            && function.declared_ret == Some(Type::Bool)
            && function.signature.as_ref().is_some_and(|params| {
                params.len() == 2 && !params[1].by_reference && params[1].ty == Some(ty)
            });
        if !fits {
            self.diagnostics.push(
                Diagnostic::error(format!("`equals` is declared `fun {ty}.equals(other in {ty}) in Bool`"))
                    .with_primary(decl.name.span, "")
                    .with_note(
                        "it gives the equality of the structure, used by `==`, `in` and the Sets (§12.5)",
                    ),
            );
            return;
        }
        if function.module != self.structs[index].module {
            self.diagnostics.push(
                Diagnostic::error(format!("the equality of `{ty}` is declared in its file"))
                    .with_primary(decl.name.span, "")
                    .with_secondary(self.structs[index].decl.name.span, "the structure is declared here")
                    .with_note("a value has one equality everywhere (§12.5, C76)"),
            );
            return;
        }
        structure.set_custom_equality();
        self.structs[index].equals = Some(method);
    }

    /// An equality reads no variable of the script: it runs wherever values are
    /// compared (C76).
    pub(crate) fn check_equalities(&mut self) {
        let mut reported = HashSet::new();
        for index in 0..self.structs.len() {
            let Some(method) = self.structs[index].equals else { continue };
            let Some(instance) = self.functions[method].instance else { continue };
            let Some(&(global, _)) = self.globals_used_by(instance).first() else { continue };
            if !reported.insert(method) {
                continue;
            }
            let name = self.global_names[&global].clone();
            self.diagnostics.push(
                Diagnostic::error(format!("`equals` cannot use `{name}`, a variable of the script"))
                    .with_primary(self.functions[method].decl.name.span, "")
                    .with_note("an equality runs wherever values are compared: it depends on the two values only (§12.5, C76)"),
            );
        }
    }
}
