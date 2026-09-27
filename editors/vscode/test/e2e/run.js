// The extension in a real VS Code, with `lion lsp`: `npm run e2e`. It opens a window of
// its own, with a profile of its own, then closes it. `LION_VSCODE` names the program
// of VS Code (by default that of the snap), `LION` the `lion` command (by default the
// one of `target/debug`).

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { runTests } = require('@vscode/test-electron');

async function main() {
  const extension = path.resolve(__dirname, '..', '..');
  const lion = process.env.LION || path.resolve(extension, '..', '..', 'target', 'debug', 'lion');
  const vscode = process.env.LION_VSCODE || '/snap/code/current/usr/share/code/code';
  const place = fs.mkdtempSync(path.join(os.tmpdir(), 'lion-vscode-'));
  const workspace = path.join(place, 'workspace');
  fs.mkdirSync(path.join(workspace, '.vscode'), { recursive: true });
  fs.writeFileSync(path.join(workspace, '.vscode', 'settings.json'), JSON.stringify({ 'lion.path': lion }));
  fs.writeFileSync(path.join(workspace, 'main.lion'), 'let x = 3 + "a"\nif x > 0:\nshow(x)\n;\n');
  try {
    await runTests({
      vscodeExecutablePath: vscode,
      extensionDevelopmentPath: extension,
      extensionTestsPath: path.join(__dirname, 'suite.js'),
      extensionTestsEnv: { LION_WORKSPACE: workspace },
      launchArgs: [
        workspace,
        '--no-sandbox',
        '--ozone-platform=x11',
        '--disable-extensions',
        '--disable-workspace-trust',
        '--skip-welcome',
        '--skip-release-notes',
        '--user-data-dir',
        path.join(place, 'user'),
        '--extensions-dir',
        path.join(place, 'extensions'),
      ],
    });
  } finally {
    fs.rmSync(place, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
