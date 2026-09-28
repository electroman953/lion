// Run inside VS Code by `run.js`: the file of the workspace, its diagnostics, its
// formatting, its outline, its names and its indentation.

const assert = require('node:assert');
const path = require('node:path');
const vscode = require('vscode');

/** The value of `probe` once it has one, checked every 100 ms for 20 s. */
async function eventually(probe, what) {
  for (let tries = 0; tries < 200; tries++) {
    const value = probe();
    if (value) {
      return value;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${what}`);
}

async function run() {
  const uri = vscode.Uri.file(path.join(process.env.LION_WORKSPACE, 'main.lion'));
  const document = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(document);
  assert.strictEqual(document.languageId, 'lion');

  const diagnostics = await eventually(() => {
    const found = vscode.languages.getDiagnostics(uri);
    return found.length > 0 && found;
  }, 'the diagnostics');
  assert.strictEqual(diagnostics.length, 1);
  assert.match(diagnostics[0].message, /^`\+` cannot be applied to Int and Text/);
  assert.strictEqual(diagnostics[0].range.start.line, 0);
  assert.strictEqual(diagnostics[0].range.start.character, 10);
  assert.strictEqual(diagnostics[0].severity, vscode.DiagnosticSeverity.Error);
  assert.strictEqual(diagnostics[0].source, 'lion');
  console.log('diagnostics: ok');

  const symbols = await vscode.commands.executeCommand('vscode.executeDocumentSymbolProvider', uri);
  assert.deepStrictEqual(symbols.map((symbol) => symbol.name), ['x']);
  console.log('outline: ok');

  const edits = await vscode.commands.executeCommand('vscode.executeFormatDocumentProvider', uri, {
    tabSize: 4,
    insertSpaces: true,
  });
  assert.strictEqual(edits.length, 1);
  const edit = new vscode.WorkspaceEdit();
  edit.set(uri, edits);
  await vscode.workspace.applyEdit(edit);
  assert.strictEqual(document.getText(), 'let x = 3 + "a"\nif x > 0:\n    show(x)\n;\n');
  console.log('formatting: ok');

  // The error corrected, the diagnostics go.
  await editor.edit((builder) => builder.replace(new vscode.Range(0, 12, 0, 15), '4'));
  await eventually(() => vscode.languages.getDiagnostics(uri).length === 0, 'no diagnostics');
  console.log('diagnostics after a change: ok');

  // The names, from the index of `lion lsp`: `x` is declared line 0, and read at
  // `if x > 0` and at `show(x)`.
  const hovers = await vscode.commands.executeCommand(
    'vscode.executeHoverProvider',
    uri,
    new vscode.Position(2, 9),
  );
  assert.match(hovers[0].contents[0].value, /let x in Int/);
  console.log('hover: ok');

  const declarations = await vscode.commands.executeCommand(
    'vscode.executeDefinitionProvider',
    uri,
    new vscode.Position(2, 9),
  );
  assert.strictEqual(declarations[0].uri.fsPath, uri.fsPath);
  assert.strictEqual(declarations[0].range.start.line, 0);
  assert.strictEqual(declarations[0].range.start.character, 4);
  console.log('go to definition: ok');

  const uses = await vscode.commands.executeCommand(
    'vscode.executeReferenceProvider',
    uri,
    new vscode.Position(0, 4),
  );
  const places = uses.map((use) => [use.range.start.line, use.range.start.character]);
  places.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  assert.deepStrictEqual(places, [[0, 4], [1, 3], [2, 9]]);
  console.log('references: ok');

  // A line that ends with `:` opens a block; `else` closes it.
  const end = document.lineAt(document.lineCount - 1).range.end;
  editor.selection = new vscode.Selection(end, end);
  await vscode.commands.executeCommand('type', { text: 'while x > 0:' });
  await vscode.commands.executeCommand('type', { text: '\n' });
  const inside = document.lineAt(editor.selection.active.line).text;
  assert.strictEqual(inside, '    ');
  console.log('indentation: ok');

  await vscode.commands.executeCommand('workbench.action.files.revert');
}

module.exports = { run };
