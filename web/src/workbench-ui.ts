/** Views that stay on the tab strip. The rest live under More views. */
export const PRIMARY_VIEWS = ["icon", "diagram", "text", "doc"] as const;
/** Flat model, Structure, Linear, and Diff. Structure is still opened by `openDebugger`. */
export const MORE_VIEWS = ["flat", "structure", "linear", "diff"] as const;

export function isMoreView(view: string): boolean {
  return (MORE_VIEWS as readonly string[]).includes(view);
}

export interface LogChrome {
  /** The log body is open. */
  expanded: boolean;
  /** The user opened it; an error also expands, until they close it. */
  userOpen: boolean;
  lastText: string;
  lastClass: string;
}

export function initialLogChrome(): LogChrome {
  return { expanded: false, userOpen: false, lastText: "", lastClass: "" };
}

/** `err` expands the log. `ok` and `warn` only colour the status line. */
export function logClassToken(cls: string): string {
  const parts = cls.split(/\s+/);
  if (parts.includes("err")) return "err";
  if (parts.includes("warn")) return "warn";
  if (parts.includes("ok")) return "ok";
  return "";
}

export function pushLogLine(state: LogChrome, text: string, cls = ""): LogChrome {
  const token = logClassToken(cls);
  return {
    userOpen: state.userOpen,
    lastText: text,
    lastClass: token,
    expanded: state.userOpen || token === "err",
  };
}

export function toggleLogChrome(state: LogChrome): LogChrome {
  const userOpen = !state.expanded;
  return { ...state, userOpen, expanded: userOpen };
}

/** Paint the one-line status and the collapsed footer. */
export function applyLogChrome(footer: HTMLElement, status: HTMLElement, state: LogChrome): void {
  footer.classList.toggle("collapsed", !state.expanded);
  status.textContent = state.lastText || "No messages";
  status.className = "log-status" + (state.lastClass ? ` ${state.lastClass}` : "");
  status.setAttribute("aria-expanded", String(state.expanded));
}

/** Diff is offered once the open document differs from the text recorded in `baseline`. */
export function diffViewEnabled(edited: boolean): boolean {
  return edited;
}
