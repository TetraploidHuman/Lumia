import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Trace,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

function lumiaPath(): string {
  return (
    vscode.workspace.getConfiguration("lumia").get<string>("path")?.trim() ||
    "lumia"
  );
}

function resolveExecutable(cmd: string): string | undefined {
  if (path.isAbsolute(cmd) && fs.existsSync(cmd)) {
    return cmd;
  }
  const home = os.homedir();
  const extras = [
    path.join(home, ".local", "bin"),
    path.join(home, ".cargo", "bin"),
  ];
  const pathEnv = [...extras, ...(process.env.PATH ?? "").split(path.delimiter)]
    .filter(Boolean)
    .filter((v, i, a) => a.indexOf(v) === i)
    .join(path.delimiter);
  for (const dir of pathEnv.split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, cmd);
    if (fs.existsSync(candidate)) {
      return candidate;
    }
    if (process.platform === "win32") {
      for (const ext of [".exe", ".cmd", ".bat"]) {
        const withExt = candidate + ext;
        if (fs.existsSync(withExt)) {
          return withExt;
        }
      }
    }
  }
  return undefined;
}

function shellQuote(s: string): string {
  if (/^[A-Za-z0-9_./:@%+=,-]+$/.test(s)) {
    return s;
  }
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  const cmd = lumiaPath();
  const resolved = resolveExecutable(cmd);
  if (!resolved) {
    void vscode.window.showErrorMessage(
      `Lumia: cannot find executable "${cmd}". Build the compiler and set lumia.path, or add it to PATH.`
    );
    return;
  }

  const trace = vscode.workspace
    .getConfiguration("lumia")
    .get<string>("lsp.trace", "off");

  const serverOptions: ServerOptions = {
    command: resolved,
    args: ["lsp"],
    options: { env: process.env },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "lumia" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.lm"),
    },
    outputChannelName: "Lumia Language Server",
  };

  client = new LanguageClient(
    "lumia",
    "Lumia Language Server",
    serverOptions,
    clientOptions
  );
  client.setTrace(
    trace === "verbose"
      ? Trace.Verbose
      : trace === "messages"
        ? Trace.Messages
        : Trace.Off
  );

  context.subscriptions.push(client);
  await client.start();
}

async function restartServer(context: vscode.ExtensionContext): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
  await startClient(context);
  void vscode.window.showInformationMessage("Lumia language server restarted.");
}

async function resolveLmPath(
  uri?: vscode.Uri
): Promise<string | undefined> {
  if (uri && uri.scheme === "file" && uri.fsPath.endsWith(".lm")) {
    return uri.fsPath;
  }
  const ed = vscode.window.activeTextEditor;
  if (
    ed &&
    (ed.document.languageId === "lumia" || ed.document.fileName.endsWith(".lm"))
  ) {
    if (ed.document.isDirty) {
      await ed.document.save();
    }
    return ed.document.uri.fsPath;
  }
  return undefined;
}

async function getLumiaTerminal(cwd?: string): Promise<vscode.Terminal> {
  const existing = vscode.window.terminals.find((t) => t.name === "Lumia");
  if (existing) {
    return existing;
  }
  return vscode.window.createTerminal({ name: "Lumia", cwd });
}

async function runCli(args: string[], cwd?: string): Promise<void> {
  const cmd = resolveExecutable(lumiaPath()) ?? lumiaPath();
  const terminal = await getLumiaTerminal(cwd);
  terminal.show(true);
  const quoted = [cmd, ...args].map(shellQuote).join(" ");
  const prefix = cwd ? `cd ${shellQuote(cwd)} && ` : "";
  terminal.sendText(`${prefix}${quoted}`);
}

/** Build current file then execute the binary (no separate `lumia run`). */
async function runFile(uri?: vscode.Uri): Promise<void> {
  const file = await resolveLmPath(uri);
  if (!file) {
    void vscode.window.showWarningMessage("Open a .lm file first.");
    return;
  }
  const cwd = path.dirname(file);
  const stem =
    path.basename(file, ".lm").replace(/[^A-Za-z0-9_]/g, "_") || "out";
  const outDir = path.join(cwd, "target", "lumia");
  const out = path.join(outDir, stem);
  const cmd = resolveExecutable(lumiaPath()) ?? lumiaPath();
  const terminal = await getLumiaTerminal(cwd);
  terminal.show(true);
  const script = [
    `mkdir -p ${shellQuote(outDir)}`,
    `${shellQuote(cmd)} build ${shellQuote(file)} -o ${shellQuote(out)}`,
    shellQuote(out),
  ].join(" && ");
  terminal.sendText(`cd ${shellQuote(cwd)} && ${script}`);
}

async function buildFile(uri?: vscode.Uri): Promise<void> {
  const file = await resolveLmPath(uri);
  if (!file) {
    void vscode.window.showWarningMessage("Open a .lm file first.");
    return;
  }
  const cwd = path.dirname(file);
  const stem =
    path.basename(file, ".lm").replace(/[^A-Za-z0-9_]/g, "_") || "out";
  const outDir = path.join(cwd, "target", "lumia");
  const out = path.join(outDir, stem);
  await runCli(["build", file, "-o", out], cwd);
}

function isLumiaEditor(
  editor: vscode.TextEditor | undefined
): boolean {
  return !!editor && (
    editor.document.languageId === "lumia" ||
    editor.document.fileName.endsWith(".lm")
  );
}

export async function activate(
  context: vscode.ExtensionContext
): Promise<void> {
  // Commands / status bar must register even if LSP client fails to load.
  const runBtn = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    100
  );
  runBtn.command = "lumia.runFile";
  runBtn.text = "$(play) Lumia Run";
  runBtn.tooltip = "Build and run the current .lm file";
  context.subscriptions.push(runBtn);

  const refreshStatus = () => {
    if (isLumiaEditor(vscode.window.activeTextEditor)) {
      runBtn.show();
    } else {
      runBtn.hide();
    }
  };
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(refreshStatus),
    vscode.workspace.onDidOpenTextDocument(refreshStatus)
  );
  refreshStatus();

  context.subscriptions.push(
    vscode.commands.registerCommand("lumia.restartServer", () =>
      restartServer(context)
    ),
    vscode.commands.registerCommand("lumia.checkFile", async (uri?: vscode.Uri) => {
      const file = await resolveLmPath(uri);
      if (!file) {
        void vscode.window.showWarningMessage("Open a .lm file first.");
        return;
      }
      await runCli(["check", file], path.dirname(file));
    }),
    vscode.commands.registerCommand("lumia.buildFile", (uri?: vscode.Uri) =>
      buildFile(uri)
    ),
    vscode.commands.registerCommand("lumia.runFile", (uri?: vscode.Uri) =>
      runFile(uri)
    ),
    vscode.commands.registerCommand("lumia.formatDocument", async () => {
      const ed = vscode.window.activeTextEditor;
      if (!ed || ed.document.languageId !== "lumia") {
        return;
      }
      await vscode.commands.executeCommand("editor.action.formatDocument");
    }),
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (doc.languageId !== "lumia") return;
      const onSave = vscode.workspace
        .getConfiguration("lumia")
        .get<boolean>("checkOnSave", false);
      if (onSave) {
        void runCli(["check", doc.uri.fsPath], path.dirname(doc.uri.fsPath));
      }
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (
        e.affectsConfiguration("lumia.path") ||
        e.affectsConfiguration("lumia.lsp.trace")
      ) {
        await restartServer(context);
      }
    })
  );

  context.subscriptions.push(
    vscode.tasks.registerTaskProvider("lumia", {
      provideTasks: () => {
        const cmd = lumiaPath();
        const mk = (
          name: string,
          args: string[],
          group?: vscode.TaskGroup
        ): vscode.Task => {
          const task = new vscode.Task(
            { type: "lumia", task: name },
            vscode.TaskScope.Workspace,
            name,
            "lumia",
            new vscode.ShellExecution(cmd, args, {
              cwd: "${fileDirname}",
            }),
            ["$lumia"]
          );
          if (group) task.group = group;
          return task;
        };
        return [
          mk("check", ["check", "${file}"], vscode.TaskGroup.Build),
          mk(
            "build",
            [
              "build",
              "${file}",
              "-o",
              "${fileDirname}/target/lumia/${fileBasenameNoExtension}",
            ],
            vscode.TaskGroup.Build
          ),
          mk("fmt", ["fmt", "${file}"]),
          mk("fmt-check", ["fmt", "${file}", "--check"]),
        ];
      },
      resolveTask: (task) => task,
    })
  );

  try {
    await startClient(context);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    void vscode.window.showErrorMessage(
      `Lumia language server failed to start: ${msg}`
    );
  }
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}
