// The Lion extension of VS Code: it starts `lion lsp`, the language server of the
// `lion` command, which gives the diagnostics, the formatting and the outline of the
// files; the highlighting and the indentation are those of `syntaxes/` and
// `language-configuration.json`.

const fs = require('fs');
const os = require('os');
const path = require('path');
const vscode = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

/** @type {LanguageClient | undefined} */
let client;

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand('lion.run', () => runInTerminal('run')),
    vscode.commands.registerCommand('lion.test', () => runInTerminal('test')),
    vscode.commands.registerCommand('lion.restartServer', async () => {
      await stop();
      await start();
    }),
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration('lion.path')) {
        await stop();
        await start();
      }
    }),
  );
  return start();
}

function deactivate() {
  return stop();
}

/** The `lion` command: the setting `lion.path`, or `lion` on the PATH, or `~/.cargo/bin/lion`. */
function lionCommand() {
  const configured = vscode.workspace.getConfiguration('lion').get('path') || 'lion';
  if (configured !== 'lion') {
    return configured.startsWith('~') ? path.join(os.homedir(), configured.slice(1)) : configured;
  }
  const names = process.platform === 'win32' ? ['lion.exe', 'lion'] : ['lion'];
  const folders = (process.env.PATH || '').split(path.delimiter).filter(Boolean);
  folders.push(path.join(os.homedir(), '.cargo', 'bin'));
  for (const folder of folders) {
    for (const name of names) {
      const candidate = path.join(folder, name);
      try {
        if (fs.statSync(candidate).isFile()) {
          return candidate;
        }
      } catch {
        // Not in this folder.
      }
    }
  }
  return 'lion';
}

async function start() {
  const command = lionCommand();
  const serverOptions = { command, args: ['lsp'] };
  const clientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'lion' },
      { scheme: 'untitled', language: 'lion' },
    ],
    synchronize: {
      // The modules read from the disk, and the packages of the project (C101).
      fileEvents: vscode.workspace.createFileSystemWatcher('**/{*.lion,lion.toml,lion.lock}'),
    },
  };
  client = new LanguageClient('lion', 'Lion', serverOptions, clientOptions);
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    const choice = await vscode.window.showErrorMessage(
      `Lion: cannot start \`${command} lsp\` (${error.message || error}). ` +
        'Install Lion with `cargo install --path crates/lion_cli`, or set `lion.path`.',
      'Open Settings',
    );
    if (choice === 'Open Settings') {
      vscode.commands.executeCommand('workbench.action.openSettings', 'lion.path');
    }
  }
}

async function stop() {
  if (client) {
    const stopping = client;
    client = undefined;
    try {
      await stopping.stop();
    } catch {
      // Already stopped.
    }
  }
}

/**
 * `lion run file` or `lion test file`, in the terminal named Lion, from the folder of the
 * file, after saving it.
 */
async function runInTerminal(verb) {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'lion') {
    vscode.window.showWarningMessage('Lion: open a Lion file first.');
    return;
  }
  const document = editor.document;
  if (document.isUntitled) {
    vscode.window.showWarningMessage('Lion: save the file first.');
    return;
  }
  await document.save();
  const folder = path.dirname(document.fileName);
  let terminal = vscode.window.terminals.find((candidate) => candidate.name === 'Lion');
  if (!terminal || terminal.exitStatus !== undefined) {
    terminal = vscode.window.createTerminal({ name: 'Lion', cwd: folder });
  }
  terminal.show(true);
  const quote = (text) =>
    process.platform === 'win32' ? `"${text}"` : `'${text.replace(/'/g, `'\\''`)}'`;
  const run = `${quote(lionCommand())} ${verb} ${quote(document.fileName)}`;
  terminal.sendText(process.platform === 'win32' ? run : `cd ${quote(folder)} && ${run}`);
}

module.exports = { activate, deactivate };
