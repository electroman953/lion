// The highlighting of `syntaxes/lion.tmLanguage.json`, read by the TextMate engine of
// VS Code: `npm test`.

const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const oniguruma = require('vscode-oniguruma');
const textmate = require('vscode-textmate');

const wasm = fs.readFileSync(require.resolve('vscode-oniguruma/release/onig.wasm')).buffer;
const engine = oniguruma.loadWASM(wasm).then(() => ({
  createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
  createOnigString: (text) => new oniguruma.OnigString(text),
}));
const registry = new textmate.Registry({
  onigLib: engine,
  loadGrammar: async () =>
    textmate.parseRawGrammar(
      fs.readFileSync(path.join(__dirname, '..', 'syntaxes', 'lion.tmLanguage.json'), 'utf8'),
      'lion.tmLanguage.json',
    ),
});

/** Each piece of the lines, with the last scope that the grammar gives it. */
async function tokens(text) {
  const grammar = await registry.loadGrammar('source.lion');
  let state = textmate.INITIAL;
  const found = [];
  for (const line of text.split('\n')) {
    const result = grammar.tokenizeLine(line, state);
    for (const token of result.tokens) {
      const piece = line.slice(token.startIndex, token.endIndex);
      if (piece.trim() !== '') {
        found.push([piece.trim(), token.scopes[token.scopes.length - 1]]);
      }
    }
    state = result.ruleStack;
  }
  return found;
}

/** The scope of the first piece `piece`. */
function scope(found, piece) {
  const token = found.find(([text]) => text === piece);
  assert.ok(token, `no piece \`${piece}\` in ${JSON.stringify(found)}`);
  return token[1];
}

test('declarations name what they declare', async () => {
  const found = await tokens('fun Student.passes() in Bool = self.grade >= 10\nstruct Point:\n    x in Int\n;');
  assert.equal(scope(found, 'fun'), 'storage.type.function.lion');
  assert.equal(scope(found, 'Student'), 'entity.name.type.lion');
  assert.equal(scope(found, 'passes'), 'entity.name.function.lion');
  assert.equal(scope(found, 'Bool'), 'support.type.primitive.lion');
  assert.equal(scope(found, 'self'), 'variable.language.self.lion');
  assert.equal(scope(found, '>='), 'keyword.operator.comparison.lion');
  assert.equal(scope(found, 'Point'), 'entity.name.type.lion');
  assert.equal(scope(found, ';'), 'punctuation.section.block.end.lion');
  const other = await tokens('let limit = 10\nvar total = 0.5e-3\nColor = {red, green}\nuse shapes.circle');
  assert.equal(scope(other, 'limit'), 'variable.other.declaration.lion');
  assert.equal(scope(other, '0.5e-3'), 'constant.numeric.float.lion');
  assert.equal(scope(other, 'Color'), 'entity.name.type.lion');
  assert.equal(scope(other, 'shapes.circle'), 'entity.name.namespace.lion');
});

test('texts hold escapes and interpolations', async () => {
  const found = await tokens('show("Bonjour {name.upper()} \\{ok\\}\\q")');
  assert.equal(scope(found, 'show'), 'support.function.builtin.lion');
  assert.equal(scope(found, 'Bonjour'), 'string.quoted.double.lion');
  assert.equal(scope(found, '{'), 'punctuation.section.interpolation.begin.lion');
  assert.equal(scope(found, 'upper'), 'entity.name.function.call.lion');
  assert.equal(scope(found, '\\{'), 'constant.character.escape.lion');
  assert.equal(scope(found, '\\q'), 'invalid.illegal.escape.lion');
  // A text that is not closed stops at the end of its line (R8).
  const open = await tokens('let t = "abc\nlet u = 1');
  assert.equal(scope(open, 'u'), 'variable.other.declaration.lion');
});

test('keywords, comments and numbers', async () => {
  const found = await tokens(
    'if x mod 2 == 0 and not done: show(0xFF) ; // even\n/* a\n block */ while true: break ;\n/// doc',
  );
  assert.equal(scope(found, 'if'), 'keyword.control.lion');
  assert.equal(scope(found, 'mod'), 'keyword.operator.word.lion');
  assert.equal(scope(found, 'not'), 'keyword.operator.word.lion');
  assert.equal(scope(found, '0xFF'), 'constant.numeric.hex.lion');
  assert.equal(scope(found, '// even'), 'comment.line.double-slash.lion');
  assert.equal(scope(found, 'block'), 'comment.block.lion');
  assert.equal(scope(found, 'true'), 'constant.language.lion');
  assert.equal(scope(found, '/// doc'), 'comment.line.documentation.lion');
});
