const fs = require("fs");
const os = require("os");
const path = require("path");
const {
  commands,
  workspace,
  window,
  Uri,
  StatusBarAlignment,
} = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

/** @type {import("vscode-languageclient/node").LanguageClient | undefined} */
let client;

/**
 * @param {import("vscode").ExtensionContext} context
 */
function activate(context) {
  context.subscriptions.push(
    commands.registerCommand("lumi.run", (uri) => runFile(uri, false)),
    commands.registerCommand("lumi.buildRun", (uri) => runFile(uri, true)),
    commands.registerCommand("lumi.checkFile", (uri) => checkFile(uri)),
    commands.registerCommand("lumi.restartServer", () => restartLsp(context))
  );

  const runBtn = window.createStatusBarItem(StatusBarAlignment.Left, 100);
  runBtn.command = "lumi.run";
  runBtn.text = "$(play) Lumi Run";
  runBtn.tooltip = "Build and run the current .lm file";
  context.subscriptions.push(runBtn);

  const buildBtn = window.createStatusBarItem(StatusBarAlignment.Left, 99);
  buildBtn.command = "lumi.buildRun";
  buildBtn.text = "$(tools) Lumi Build";
  buildBtn.tooltip = "Build the current .lm file to target/lumi/";
  context.subscriptions.push(buildBtn);

  const refreshStatus = () => {
    const editor = window.activeTextEditor;
    const isLm =
      editor &&
      (editor.document.languageId === "lumi" ||
        editor.document.fileName.endsWith(".lm"));
    if (isLm) {
      runBtn.show();
      buildBtn.show();
    } else {
      runBtn.hide();
      buildBtn.hide();
    }
  };
  context.subscriptions.push(
    window.onDidChangeActiveTextEditor(refreshStatus),
    workspace.onDidOpenTextDocument(refreshStatus)
  );
  refreshStatus();

  startLsp(context);
}

/**
 * @param {import("vscode").ExtensionContext} context
 */
function startLsp(context) {
  const config = workspace.getConfiguration("lumi");
  if (!config.get("lsp.enabled", true)) {
    return;
  }

  const t0 = Date.now();
  const command = resolveLumiLsp();
  const lspEnv = { ...process.env, PATH: pathEnvWithCargo() };
  // Executable without `transport` uses stdio pipes; do NOT set TransportKind.stdio
  // or the client appends a bare `--stdio` argv that older binaries rejected.
  const serverOptions = {
    command,
    args: ["lsp"],
    options: { env: lspEnv },
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "lumi" }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.lm"),
    },
    outputChannelName: "Lumi Language Server",
  };

  client = new LanguageClient(
    "lumi",
    "Lumi Language Server",
    serverOptions,
    clientOptions
  );

  context.subscriptions.push({
    dispose: () => {
      if (client) {
        return client.stop();
      }
    },
  });

  const ch = client.outputChannel;
  if (ch) {
    ch.appendLine(
      `[lumi] starting LSP: ${command} lsp (activate+${Date.now() - t0}ms)`
    );
  }

  client
    .start()
    .then(() => {
      if (ch) {
        ch.appendLine(
          `[lumi] LSP ready in ${Date.now() - t0}ms (command: ${command})`
        );
      }
    })
    .catch((err) => {
      window.showErrorMessage(
        `Lumi LSP failed to start (${command} lsp). Set lumi.lsp.path or add lumi to PATH. (${err})`
      );
    });
}

/**
 * @param {import("vscode").ExtensionContext} context
 */
async function restartLsp(context) {
  if (client) {
    try {
      await client.stop();
    } catch {
      /* ignore */
    }
    client = undefined;
  }
  startLsp(context);
  window.showInformationMessage("Lumi language server restarted.");
}

function resolveLumi() {
  const configured = workspace
    .getConfiguration("lumi")
    .get("lsp.path", "")
    .trim();
  const home = os.homedir();
  const candidates = [
    configured,
    path.join(home, ".local", "bin", "lumi"),
    path.join(home, ".cargo", "bin", "lumi"),
  ];
  for (const c of candidates) {
    if (c && looksLikePath(c) && isExecutableFile(c)) {
      return c;
    }
  }
  if (configured && !looksLikePath(configured)) {
    return configured;
  }
  return path.join(home, ".local", "bin", "lumi");
}

/** Prefer the slim no-LLVM LSP binary for fast cold start. */
function resolveLumiLsp() {
  const home = os.homedir();
  const slim = path.join(home, ".local", "lib", "lumi", "lumi-lsp");
  const configured = workspace
    .getConfiguration("lumi")
    .get("lsp.path", "")
    .trim();

  // Wrapper / PATH name `lumi` still routes to the fat binary for `build`;
  // for LSP always prefer the slim binary when present.
  const looksLikeWrapper =
    !configured ||
    configured === "lumi" ||
    configured.endsWith(`${path.sep}bin${path.sep}lumi`) ||
    configured.endsWith("/bin/lumi");
  if (looksLikeWrapper && isExecutableFile(slim)) {
    return slim;
  }

  if (configured) {
    if (looksLikePath(configured) && isExecutableFile(configured)) {
      return configured;
    }
    if (!looksLikePath(configured)) {
      return configured;
    }
  }
  if (isExecutableFile(slim)) {
    return slim;
  }
  return resolveLumi();
}

/** @param {string} p */
function looksLikePath(p) {
  return (
    path.isAbsolute(p) ||
    p.includes("/") ||
    p.includes("\\") ||
    p.startsWith(".")
  );
}

/** @param {string} p */
function isExecutableFile(p) {
  try {
    fs.accessSync(p, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function pathEnvWithCargo() {
  const home = os.homedir();
  const extras = [
    path.join(home, ".local", "bin"),
    path.join(home, ".cargo", "bin"),
  ];
  const existing = process.env.PATH || "";
  return [...extras, ...existing.split(path.delimiter)]
    .filter(Boolean)
    .filter((v, i, a) => a.indexOf(v) === i)
    .join(path.delimiter);
}

/**
 * @param {import("vscode").Uri | undefined} uri
 * @param {boolean} buildOnly
 */
async function runFile(uri, buildOnly) {
  const filePath = await resolveLmPath(uri);
  if (!filePath) {
    window.showErrorMessage(
      "Open a .lm file first, or select one in the explorer."
    );
    return;
  }

  const lumi = resolveLumi();
  const cwd =
    workspace.getWorkspaceFolder(Uri.file(filePath))?.uri.fsPath ||
    path.dirname(filePath);

  const stem =
    path.basename(filePath, path.extname(filePath)).replace(/[^A-Za-z0-9_]/g, "_") ||
    "out";
  const outAbs = path.join(path.dirname(filePath), "target", "lumi", stem);

  const term =
    window.terminals.find((t) => t.name === "Lumi") ||
    window.createTerminal({
      name: "Lumi",
      cwd,
      env: { ...process.env, PATH: pathEnvWithCargo() },
    });
  term.show(true);

  const mkdir = `mkdir -p ${shellQuote(path.dirname(outAbs))}`;
  const build = [
    shellQuote(lumi),
    "build",
    shellQuote(filePath),
    "-o",
    shellQuote(outAbs),
  ].join(" ");
  const script = buildOnly
    ? `${mkdir} && ${build}`
    : `${mkdir} && ${build} && ${shellQuote(outAbs)}`;
  term.sendText(`cd ${shellQuote(cwd)} && ${script}`);
}

/**
 * @param {import("vscode").Uri | undefined} uri
 */
async function checkFile(uri) {
  const filePath = await resolveLmPath(uri);
  if (!filePath) {
    window.showErrorMessage("Open a .lm file first.");
    return;
  }
  const lumi = resolveLumi();
  const cwd =
    workspace.getWorkspaceFolder(Uri.file(filePath))?.uri.fsPath ||
    path.dirname(filePath);
  const term =
    window.terminals.find((t) => t.name === "Lumi") ||
    window.createTerminal({
      name: "Lumi",
      cwd,
      env: { ...process.env, PATH: pathEnvWithCargo() },
    });
  term.show(true);
  const quoted = [lumi, "check", filePath].map(shellQuote).join(" ");
  term.sendText(`cd ${shellQuote(cwd)} && ${quoted}`);
}

/**
 * @param {import("vscode").Uri | undefined} uri
 * @returns {Promise<string | undefined>}
 */
async function resolveLmPath(uri) {
  if (uri && uri.scheme === "file" && uri.fsPath.endsWith(".lm")) {
    return uri.fsPath;
  }
  const editor = window.activeTextEditor;
  if (
    editor &&
    (editor.document.languageId === "lumi" ||
      editor.document.fileName.endsWith(".lm"))
  ) {
    if (editor.document.isDirty) {
      await editor.document.save();
    }
    return editor.document.uri.fsPath;
  }
  return undefined;
}

/** @param {string} s */
function shellQuote(s) {
  if (/^[A-Za-z0-9_./:@%+=,-]+$/.test(s)) {
    return s;
  }
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

function deactivate() {
  if (!client) {
    return undefined;
  }
  return client.stop();
}

module.exports = { activate, deactivate };
