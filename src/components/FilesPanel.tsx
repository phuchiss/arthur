import { useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import hljs from "highlight.js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, ChangedFile, FilePreview, FileStatus } from "../lib/ipc";
import { Icon } from "./Icon";

type Tab = "preview" | "diff";
type Mode = "changed" | "all";

const STATUS_GLYPH: Record<FileStatus, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  untracked: "U",
  unchanged: "·",
};

const STATUS_LABEL: Record<FileStatus, string> = {
  modified: "modified",
  added: "added",
  deleted: "deleted",
  renamed: "renamed",
  untracked: "untracked",
  unchanged: "unchanged",
};

type Cached = { preview?: FilePreview | { error: string }; diff?: string | { error: string } };

const MIN_PANEL_W = 280;
const MAX_PANEL_W = 1100;
const MIN_LIST_H = 80;
const MIN_VIEW_H = 120;
const WIDTH_KEY = "arthur.filesPanel.width";
const LIST_KEY = "arthur.filesPanel.listHeight";

const clamp = (n: number, lo: number, hi: number) => Math.min(Math.max(n, lo), hi);

export function FilesPanel({
  sessionKey,
  projectDir,
  refreshNonce,
  open,
  onClose,
  onCountChange,
}: {
  sessionKey: string;
  projectDir: string;
  /** Bump to force a re-fetch of the changed-file list. */
  refreshNonce: number;
  open: boolean;
  onClose: () => void;
  /** Reports the changed-file count back so the toggle button can show a badge. */
  onCountChange?: (count: number) => void;
}) {
  const [files, setFiles] = useState<ChangedFile[]>([]);
  const [gitAvailable, setGitAvailable] = useState(true);
  const [truncated, setTruncated] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("diff");
  const [mode, setMode] = useState<Mode>("changed");
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [cache, setCache] = useState<Record<string, Cached>>({});
  const [busy, setBusy] = useState(false);
  const [panelWidth, setPanelWidth] = useState<number>(() => {
    const v = Number(localStorage.getItem(WIDTH_KEY));
    return v >= MIN_PANEL_W && v <= MAX_PANEL_W ? v : 380;
  });
  // null → fall back to the CSS default split (35% list); a number pins the
  // list height in px after the user has dragged the inner divider.
  const [listHeight, setListHeight] = useState<number | null>(() => {
    const v = Number(localStorage.getItem(LIST_KEY));
    return v >= MIN_LIST_H ? v : null;
  });
  const bodyRef = useRef<HTMLDivElement | null>(null);

  // Drag the panel's left edge. The panel is docked right, so moving the cursor
  // left (decreasing clientX) widens it.
  const startWidthDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = panelWidth;
    const max = Math.min(MAX_PANEL_W, window.innerWidth - 320);
    const onMove = (ev: MouseEvent) =>
      setPanelWidth(clamp(startW + (startX - ev.clientX), MIN_PANEL_W, max));
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.classList.remove("is-resizing-col");
      setPanelWidth((w) => {
        localStorage.setItem(WIDTH_KEY, String(Math.round(w)));
        return w;
      });
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    document.body.classList.add("is-resizing-col");
  };

  // Drag the divider between the file list and the diff/preview view.
  const startListDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    const body = bodyRef.current;
    if (!body) return;
    const rect = body.getBoundingClientRect();
    const onMove = (ev: MouseEvent) =>
      setListHeight(clamp(ev.clientY - rect.top, MIN_LIST_H, rect.height - MIN_VIEW_H));
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.classList.remove("is-resizing-row");
      setListHeight((h) => {
        if (h != null) localStorage.setItem(LIST_KEY, String(Math.round(h)));
        return h;
      });
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    document.body.classList.add("is-resizing-row");
  };

  // Pull the changed-file list whenever the panel becomes visible, the session
  // changes, or a turn/step bumps refreshNonce. Drop the per-file cache too —
  // it's stale relative to the new working-tree state.
  useEffect(() => {
    if (!sessionKey || !projectDir) return;
    let cancelled = false;
    setBusy(true);
    const fetchList =
      mode === "all"
        ? api.listAllFiles(sessionKey, projectDir)
        : api.listChangedFiles(sessionKey, projectDir);
    fetchList
      .then((res) => {
        if (cancelled) return;
        setFiles(res.files);
        setGitAvailable(res.git_available);
        setTruncated(res.truncated);
        setCache({});
        onCountChange?.(res.files.length);
        // Keep the current selection if it survived; otherwise pick the first.
        setSelected((prev) => {
          if (prev && res.files.some((f) => f.path === prev)) return prev;
          return res.files[0]?.path ?? null;
        });
      })
      .catch(() => {
        if (cancelled) return;
        setFiles([]);
        setGitAvailable(false);
        onCountChange?.(0);
      })
      .finally(() => {
        if (!cancelled) setBusy(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionKey, projectDir, refreshNonce, mode]);

  // Fetch preview + diff in parallel the first time a file is shown in this
  // refresh cycle. Errors get stored as `{ error }` so we don't retry on every
  // tab switch.
  useEffect(() => {
    if (!open || !selected) return;
    const existing = cache[selected];
    if (existing?.preview && existing?.diff) return;

    let cancelled = false;
    const previewP = existing?.preview
      ? Promise.resolve(existing.preview)
      : api
          .readFilePreview(projectDir, selected)
          .catch((e): { error: string } => ({ error: String(e) }));
    const diffP = existing?.diff
      ? Promise.resolve(existing.diff)
      : api
          .diffFile(sessionKey, projectDir, selected)
          .catch((e): { error: string } => ({ error: String(e) }));

    Promise.all([previewP, diffP]).then(([preview, diff]) => {
      if (cancelled) return;
      setCache((prev) => ({ ...prev, [selected]: { preview, diff } }));
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, selected, projectDir, sessionKey]);

  const resetBaseline = async () => {
    if (!sessionKey || !projectDir) return;
    try {
      await api.resetFilesBaseline(sessionKey, projectDir);
    } catch {
      // ignore — list refresh below will surface any real problem
    }
    // Trigger a refresh without waiting on a parent nonce bump.
    setCache({});
    setBusy(true);
    const res = await (mode === "all"
      ? api.listAllFiles(sessionKey, projectDir)
      : api.listChangedFiles(sessionKey, projectDir)
    ).catch(() => null);
    setBusy(false);
    if (!res) return;
    setFiles(res.files);
    setGitAvailable(res.git_available);
    setTruncated(res.truncated);
    onCountChange?.(res.files.length);
    setSelected(res.files[0]?.path ?? null);
  };

  const selectedCache = selected ? cache[selected] : undefined;
  const tree = useMemo(() => buildTree(files), [files]);
  const toggleDir = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  if (!open) return null;

  return (
    <aside className="files-panel" style={{ width: panelWidth }}>
      <div
        className="files-resize files-resize--col"
        onMouseDown={startWidthDrag}
        title="Drag to resize panel"
      />
      <div className="files-panel__head">
        <div className="files-panel__title">
          <Icon name="folder" size={12} />
          <span>Files</span>
          <span className="files-panel__count">{files.length}</span>
          {truncated && <span className="files-panel__count" title="More files exist; list capped">+</span>}
        </div>
        <div className="files-panel__head-right">
          <div className="files-panel__modes" role="tablist">
            <button
              className={`files-tab${mode === "changed" ? " is-on" : ""}`}
              onClick={() => setMode("changed")}
              title="Only files changed since the baseline"
            >
              Changed
            </button>
            <button
              className={`files-tab${mode === "all" ? " is-on" : ""}`}
              onClick={() => setMode("all")}
              title="Every file in the project"
            >
              All
            </button>
          </div>
          <button
            className="composer__pill"
            title="Set baseline to current HEAD — only changes after this point will appear"
            onClick={resetBaseline}
            disabled={busy || !gitAvailable}
          >
            <Icon name="refresh" size={11} /> reset
          </button>
          <button className="icon-btn" onClick={onClose} title="Close files panel">
            <Icon name="x" size={12} />
          </button>
        </div>
      </div>

      {!gitAvailable ? (
        <div className="files-panel__empty">
          Not a git repo — initialise one to track changes.
        </div>
      ) : files.length === 0 ? (
        <div className="files-panel__empty">
          {busy
            ? "Scanning…"
            : mode === "all"
              ? "No files in this project."
              : "No changes since baseline."}
        </div>
      ) : (
        <div
          className="files-panel__body"
          ref={bodyRef}
          style={listHeight != null ? { gridTemplateRows: `${listHeight}px 6px 1fr` } : undefined}
        >
          {mode === "all" ? (
            <div className="files-panel__list files-panel__tree">
              {tree.map((node) => (
                <TreeRow
                  key={node.path}
                  node={node}
                  depth={0}
                  selected={selected}
                  collapsed={collapsed}
                  onToggleDir={toggleDir}
                  onSelect={setSelected}
                />
              ))}
            </div>
          ) : (
            <ul className="files-panel__list">
              {files.map((f) => (
                <li
                  key={f.path}
                  className={`files-row status-${f.status}${selected === f.path ? " is-on" : ""}`}
                  onClick={() => setSelected(f.path)}
                  title={`${STATUS_LABEL[f.status]} · ${f.path}`}
                >
                  <span className="files-row__icon">{STATUS_GLYPH[f.status]}</span>
                  <span className="files-row__path">{f.path}</span>
                </li>
              ))}
            </ul>
          )}
          <div
            className="files-resize files-resize--row"
            onMouseDown={startListDrag}
            title="Drag to resize"
          />
          <div className="files-panel__view">
            <div className="files-panel__tabs">
              <button
                className={`files-tab${tab === "diff" ? " is-on" : ""}`}
                onClick={() => setTab("diff")}
              >
                Diff
              </button>
              <button
                className={`files-tab${tab === "preview" ? " is-on" : ""}`}
                onClick={() => setTab("preview")}
              >
                Preview
              </button>
              <span className="files-panel__spacer" />
              {selected && <span className="files-panel__path">{selected}</span>}
            </div>
            <div className="files-panel__pane">
              {!selected ? (
                <div className="files-panel__empty">Select a file to inspect.</div>
              ) : tab === "diff" ? (
                <DiffPane diff={selectedCache?.diff} />
              ) : (
                <PreviewPane preview={selectedCache?.preview} path={selected} />
              )}
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}

const mdComponents = {
  a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href) openUrl(href).catch(() => {});
      }}
    >
      {children}
    </a>
  ),
};

function PreviewPane({ preview, path }: { preview: Cached["preview"]; path: string }) {
  if (preview === undefined) return <div className="files-panel__empty">Loading…</div>;
  if ("error" in preview) return <div className="files-panel__empty err">{preview.error}</div>;
  if (preview.image) {
    return (
      <div className="files-image">
        <img src={preview.image} alt={path} />
      </div>
    );
  }
  if (preview.binary) {
    return (
      <div className="files-panel__empty">
        {preview.truncated ? "Image too large to preview." : "Binary file — preview omitted."}
      </div>
    );
  }
  if (!preview.content.trim() && !preview.truncated) {
    return <div className="files-panel__empty">Empty file.</div>;
  }

  if (isMarkdown(path)) {
    return (
      <div className="files-md chat-md">
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
          {preview.content}
        </ReactMarkdown>
        {preview.truncated && <span className="files-preview__trunc">… truncated …</span>}
      </div>
    );
  }

  return <CodePane content={preview.content} path={path} truncated={preview.truncated} />;
}

function CodePane({
  content,
  path,
  truncated,
}: {
  content: string;
  path: string;
  truncated: boolean;
}) {
  const { html, count } = useMemo(() => {
    const lang = langForPath(path);
    const source = lang === "json" ? prettyJson(content) : content;
    let rendered: string;
    try {
      rendered =
        lang && hljs.getLanguage(lang)
          ? hljs.highlight(source, { language: lang }).value
          : hljs.highlightAuto(source).value;
    } catch {
      rendered = escapeHtml(source);
    }
    return { html: rendered, count: source.split("\n").length };
  }, [content, path]);

  return (
    <div className="files-code">
      <div className="files-code__gutter" aria-hidden="true">
        {Array.from({ length: count }, (_, i) => (
          <span key={i}>{i + 1}</span>
        ))}
      </div>
      <pre className="files-code__body hljs">
        <code dangerouslySetInnerHTML={{ __html: html }} />
        {truncated && <span className="files-preview__trunc">… truncated …</span>}
      </pre>
    </div>
  );
}

function isMarkdown(path: string): boolean {
  return /\.(md|markdown|mdx)$/i.test(path);
}

/** Pretty-print JSON for preview; fall back to the raw text if it won't parse. */
function prettyJson(content: string): string {
  try {
    return JSON.stringify(JSON.parse(content), null, 2);
  } catch {
    return content;
  }
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Map a file extension to a highlight.js language id. Unknown extensions
 *  return undefined so the caller falls back to auto-detection. */
function langForPath(path: string): string | undefined {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return EXT_LANG[ext];
}

const EXT_LANG: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  rs: "rust",
  py: "python",
  rb: "ruby",
  go: "go",
  java: "java",
  kt: "kotlin",
  swift: "swift",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  json: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "ini",
  ini: "ini",
  css: "css",
  scss: "scss",
  less: "less",
  html: "xml",
  xml: "xml",
  svg: "xml",
  sql: "sql",
  lua: "lua",
  dockerfile: "dockerfile",
};

function DiffPane({ diff }: { diff: Cached["diff"] }) {
  // Run the parse unconditionally so we don't violate the rules of hooks; it
  // returns an empty list for the loading/error cases.
  const lines = useMemo(
    () => (typeof diff === "string" ? parseDiff(diff) : []),
    [diff]
  );
  if (diff === undefined) return <div className="files-panel__empty">Loading…</div>;
  if (typeof diff !== "string") return <div className="files-panel__empty err">{diff.error}</div>;
  if (lines.length === 0) {
    return <div className="files-panel__empty">No diff (file matches baseline).</div>;
  }
  return (
    <div className="files-diff">
      {lines.map((l, i) => (
        <div key={i} className={`diff-line ${l.kind}`}>
          {l.text}
        </div>
      ))}
    </div>
  );
}

type DiffLineKind = "add" | "del" | "hunk" | "ctx" | "meta";
type DiffLine = { kind: DiffLineKind; text: string };

function parseDiff(diff: string): DiffLine[] {
  const out: DiffLine[] = [];
  for (const raw of diff.split("\n")) {
    if (raw.startsWith("@@")) out.push({ kind: "hunk", text: raw });
    else if (raw.startsWith("+++") || raw.startsWith("---") || raw.startsWith("diff ") || raw.startsWith("index ") || raw.startsWith("new file") || raw.startsWith("deleted file"))
      out.push({ kind: "meta", text: raw });
    else if (raw.startsWith("+")) out.push({ kind: "add", text: raw });
    else if (raw.startsWith("-")) out.push({ kind: "del", text: raw });
    else out.push({ kind: "ctx", text: raw });
  }
  // Trim a single trailing blank line that's almost always present in `git diff` output.
  if (out.length > 0 && out[out.length - 1].kind === "ctx" && out[out.length - 1].text === "") {
    out.pop();
  }
  return out;
}

type TreeNode = {
  name: string;
  /** Full slash-path; the diff/preview key for files, the collapse key for dirs. */
  path: string;
  /** Set iff this node is a file. Directories leave it undefined. */
  status?: FileStatus;
  children: TreeNode[];
};

/** Fold a flat, path-sorted file list into a nested directory tree. Dirs sort
 *  before files; both alphabetically within a level. */
function buildTree(files: ChangedFile[]): TreeNode[] {
  const root: TreeNode = { name: "", path: "", children: [] };
  for (const f of files) {
    const parts = f.path.split("/");
    let node = root;
    parts.forEach((part, i) => {
      const childPath = parts.slice(0, i + 1).join("/");
      let child = node.children.find((c) => c.path === childPath);
      if (!child) {
        child = { name: part, path: childPath, children: [] };
        node.children.push(child);
      }
      if (i === parts.length - 1) child.status = f.status;
      node = child;
    });
  }
  const sortRec = (n: TreeNode) => {
    n.children.sort((a, b) => {
      const aDir = a.status === undefined ? 0 : 1;
      const bDir = b.status === undefined ? 0 : 1;
      if (aDir !== bDir) return aDir - bDir;
      return a.name.localeCompare(b.name);
    });
    n.children.forEach(sortRec);
  };
  sortRec(root);
  return root.children;
}

function TreeRow({
  node,
  depth,
  selected,
  collapsed,
  onToggleDir,
  onSelect,
}: {
  node: TreeNode;
  depth: number;
  selected: string | null;
  collapsed: Set<string>;
  onToggleDir: (path: string) => void;
  onSelect: (path: string) => void;
}) {
  const indent = { paddingLeft: `${6 + depth * 12}px` };
  if (node.status === undefined) {
    const isCollapsed = collapsed.has(node.path);
    return (
      <>
        <div
          className="files-row files-row--dir"
          style={indent}
          onClick={() => onToggleDir(node.path)}
          title={node.path}
        >
          <span className="files-row__caret">
            <Icon name={isCollapsed ? "chev-right" : "chev-down"} size={11} />
          </span>
          <span className="files-row__name">{node.name}</span>
        </div>
        {!isCollapsed &&
          node.children.map((child) => (
            <TreeRow
              key={child.path}
              node={child}
              depth={depth + 1}
              selected={selected}
              collapsed={collapsed}
              onToggleDir={onToggleDir}
              onSelect={onSelect}
            />
          ))}
      </>
    );
  }
  return (
    <div
      className={`files-row status-${node.status}${selected === node.path ? " is-on" : ""}`}
      style={indent}
      onClick={() => onSelect(node.path)}
      title={`${STATUS_LABEL[node.status]} · ${node.path}`}
    >
      <span className="files-row__icon">{STATUS_GLYPH[node.status]}</span>
      <span className="files-row__name">{node.name}</span>
    </div>
  );
}
