// In-app assistant: Claude with tools over the workbench (browse, read and edit models,
// check, simulate, and the analysis reports). The API key is the user's own; it stays in
// this browser and requests go straight from the page to the Claude API.
import type { BetaMessageParam, BetaToolResultBlockParam, BetaToolUnion } from "@anthropic-ai/sdk/resources/beta/messages/messages";
import "./assistant.css";
import { api } from "./rpc";
import type { ClassEntry, SimSettings, SimSummary } from "./types";

const MODEL = "claude-opus-5-5";
const KEY_STORE = "daedalus.assistant.key";

/** What the assistant needs from the rest of the workbench. */
export interface AssistantHooks {
  currentClass(): string | null;
  openClass(path: string): Promise<void>;
  /** Called after a tool changed a document, so views and the package tree refresh. */
  afterEdit(uri: string): Promise<void>;
  /** Called after a tool simulated a model, so the variable browser shows that result. */
  afterSimulate(model: string, summary: SimSummary): void;
}

/** A tool the assistant can call: JSON schema plus a handler returning text for the model. */
export interface AssistantTool {
  name: string;
  description: string;
  input_schema: { type: "object"; properties: Record<string, unknown>; required?: string[] };
  run(input: Record<string, unknown>): Promise<string>;
}

const extraTools: AssistantTool[] = [];
/** Let other workbench features (structure, sensitivity, profiling) offer tools. */
export function registerAssistantTool(t: AssistantTool) {
  extraTools.push(t);
}

const num = (x: unknown): number | undefined => (typeof x === "number" && isFinite(x) ? x : undefined);
const str = (x: unknown): string | undefined => (typeof x === "string" ? x : undefined);

function fmt(x: number): string {
  if (x === 0 || (Math.abs(x) >= 1e-3 && Math.abs(x) < 1e5)) return String(+x.toPrecision(7));
  return x.toExponential(4);
}

function findEntry(entries: ClassEntry[], path: string): ClassEntry | null {
  for (const e of entries) {
    if (e.path === path) return e;
    if (path.startsWith(e.path + ".")) {
      const f = findEntry(e.children, path);
      if (f) return f;
    }
  }
  return null;
}

function listEntries(entries: ClassEntry[], depth: number, indent = 0, out: string[] = []): string[] {
  for (const e of entries) {
    out.push(`${"  ".repeat(indent)}${e.kind} ${e.path}${e.runnable ? " [runnable]" : ""}${e.description ? ` — ${e.description}` : ""}`);
    if (depth > 1) listEntries(e.children, depth - 1, indent + 1, out);
  }
  return out;
}

function builtinTools(hooks: AssistantHooks): AssistantTool[] {
  return [
    {
      name: "list_classes",
      description: "List the classes under a package of the loaded libraries (Daedalus, the Modelica Standard Library, the user's files). Empty package lists the top level. Runnable models have an experiment annotation.",
      input_schema: { type: "object", properties: { package: { type: "string" }, depth: { type: "integer" } } },
      async run(i) {
        const tree = await api.classTree();
        const pkg = str(i.package) ?? "";
        const depth = Math.max(1, Math.min(3, num(i.depth) ?? 1));
        if (!pkg) return listEntries(tree, depth).join("\n");
        const e = findEntry(tree, pkg);
        if (!e) throw new Error(`no class '${pkg}'`);
        return listEntries(e.children.length ? e.children : [e], depth).join("\n");
      },
    },
    {
      name: "get_class",
      description: "Modelica source text of a class, with the file it lives in.",
      input_schema: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
      async run(i) {
        const path = str(i.path);
        if (!path) throw new Error("missing path");
        const info = await api.classInfo(path);
        if (!info) throw new Error(`no class '${path}'`);
        let out = `${info.kind} ${info.path}${info.description ? ` — ${info.description}` : ""}\n`;
        if (info.source) {
          const text = await api.documentText(info.source.uri);
          out += `file ${info.source.uri} (line ${info.source.line})\n\n${text?.slice(info.source.start, info.source.end) ?? ""}`;
        }
        return out;
      },
    },
    {
      name: "read_file",
      description: "Full text of a loaded file, by its uri (as get_class reports it).",
      input_schema: { type: "object", properties: { uri: { type: "string" } }, required: ["uri"] },
      async run(i) {
        const t = await api.documentText(str(i.uri) ?? "");
        if (t == null) throw new Error("no such file");
        return t;
      },
    },
    {
      name: "write_file",
      description: "Create or replace a user model file (uri must start with user/, e.g. user/MyModels.mo) with the complete new text. Library files are read-only: copy a class into a user file to change it. Returns parse errors; always check_model afterwards.",
      input_schema: { type: "object", properties: { uri: { type: "string" }, text: { type: "string" } }, required: ["uri", "text"] },
      async run(i) {
        const uri = str(i.uri) ?? "";
        const text = str(i.text);
        if (!/^user\/[\w./-]+\.mo$/.test(uri) || uri.includes("..")) throw new Error("uri must look like user/Name.mo");
        if (text == null) throw new Error("missing text");
        const errs = await api.setDocuments([[uri, text]]);
        await hooks.afterEdit(uri);
        const e = errs[uri] ?? [];
        return e.length ? `${uri} saved with parse errors:\n${e.join("\n")}` : `${uri} saved; it parses without errors.`;
      },
    },
    {
      name: "check_model",
      description: "Translate a model and report states, algebraic loops and warnings, or the translation error.",
      input_schema: { type: "object", properties: { model: { type: "string" } }, required: ["model"] },
      async run(i) {
        const model = str(i.model) ?? "";
        const r = await api.check(model);
        if (!r.ok) throw new Error(r.error ?? "translation failed");
        const st = r.stats ?? {};
        return [
          `${model} translates: ${r.states} states, ${st.equations_after_alias ?? "?"} equations after alias elimination,`,
          `${st.algebraic_loops ?? 0} algebraic loops (largest ${st.largest_loop ?? 0}), ${st.dummy_derivatives ?? 0} dummy derivatives.`,
          ...(r.warnings ?? []).map((w) => `warning: ${w}`),
        ].join("\n");
      },
    },
    {
      name: "simulate_model",
      description: "Simulate a model and summarize: solver statistics, and for each requested variable its final, minimum and maximum value plus a short table over time. The result also appears in the workbench's variable browser.",
      input_schema: {
        type: "object",
        properties: {
          model: { type: "string" },
          stop_time: { type: "number" },
          tolerance: { type: "number" },
          method: { type: "string", description: "Integrator name as in the simulation setup" },
          parameters: { type: "object", additionalProperties: { type: "number" } },
          variables: { type: "array", items: { type: "string" } },
          points: { type: "integer" },
        },
        required: ["model"],
      },
      async run(i) {
        const model = str(i.model) ?? "";
        const s: SimSettings = {};
        if (num(i.stop_time) !== undefined) s.stop_time = num(i.stop_time);
        if (num(i.tolerance) !== undefined) s.tolerance = num(i.tolerance);
        if (str(i.method)) s.method = str(i.method);
        if (i.parameters && typeof i.parameters === "object")
          s.params = Object.entries(i.parameters as Record<string, unknown>).filter(([, v]) => num(v) !== undefined).map(([k, v]) => [k, v as number]);
        const r = await api.simulate(model, s);
        hooks.afterSimulate(model, r);
        const out: string[] = [r.ok ? `simulation of ${model} finished.` : `simulation of ${model} FAILED: ${r.error}`];
        if (r.stats) out.push(`  ${r.stats.states} states, ${r.stats.solver.steps} steps (${r.stats.solver.rejected} rejected), ${r.stats.events} events, ${r.stats.sim_ms.toFixed(1)} ms`);
        for (const l of (r.log ?? []).slice(0, 20)) out.push(`log: ${l}`);
        for (const w of r.warnings ?? []) out.push(`warning: ${w}`);
        if (!r.vars?.length) return out.join("\n");
        let names = Array.isArray(i.variables) ? (i.variables as unknown[]).filter((x): x is string => typeof x === "string") : [];
        if (!names.length) names = r.vars.filter((v) => v.kind === "series").slice(0, 6).map((v) => v.name);
        const known = names.filter((n) => r.vars.some((v) => v.name === n));
        for (const n of names) if (!known.includes(n)) out.push(`${n}: not in the result`);
        if (!known.length) return out.join("\n");
        const { time, cols } = await api.values(known);
        known.forEach((n, k) => {
          const y = cols[k];
          let lo = Infinity, hi = -Infinity;
          for (const v of y) (lo = Math.min(lo, v)), (hi = Math.max(hi, v));
          out.push(`${n}: final ${fmt(y[y.length - 1])}, min ${fmt(lo)}, max ${fmt(hi)}`);
        });
        const pts = Math.max(2, Math.min(100, num(i.points) ?? 11));
        out.push("", ["time", ...known].join("\t"));
        const t0 = time[0], t1 = time[time.length - 1];
        for (let k = 0; k < pts; k++) {
          const tq = t0 + ((t1 - t0) * k) / (pts - 1);
          let j = 0;
          while (j < time.length - 1 && time[j] < tq) j++;
          out.push([fmt(time[j]), ...cols.map((c) => fmt(c[j]))].join("\t"));
        }
        return out.join("\n");
      },
    },
    {
      name: "model_structure",
      description: "Structure report of a model: variable roles, BLT blocks and algebraic loops, the state-selection sets and why each state was chosen, and the state Jacobian (eigenvalues, time constants, participation factors, stiffness ratio, suggested solver) at the start or after simulating to `at`. `states` tries stateSelect overrides such as {\"body.v\": \"prefer\"} without editing the model.",
      input_schema: {
        type: "object",
        properties: {
          model: { type: "string" },
          at: { type: "number" },
          states: { type: "object", additionalProperties: { type: "string", enum: ["always", "prefer", "default", "avoid", "never"] } },
        },
        required: ["model"],
      },
      async run(i) {
        const opts: Record<string, unknown> = {};
        if (num(i.at) !== undefined) opts.at = num(i.at);
        if (i.states && typeof i.states === "object") opts.stateOverrides = Object.entries(i.states as Record<string, unknown>).map(([k, v]) => [k, String(v)]);
        const r = (await api.structure(str(i.model) ?? "", opts)) as { ok: boolean; error?: string; text?: string };
        if (!r.ok) throw new Error(r.error ?? "structure failed");
        const t = r.text ?? "";
        return t.length > 40000 ? t.slice(0, 40000) + "\n… truncated" : t;
      },
    },
    {
      name: "profile_simulation",
      description: "Simulate with the performance profiler: time split, which states limit the step size, the costliest blocks, events and chattering relations, with hints.",
      input_schema: { type: "object", properties: { model: { type: "string" }, stop_time: { type: "number" }, parameters: { type: "object", additionalProperties: { type: "number" } } }, required: ["model"] },
      async run(i) {
        const model = str(i.model) ?? "";
        const s: SimSettings = { profile: true };
        if (num(i.stop_time) !== undefined) s.stop_time = num(i.stop_time);
        if (i.parameters && typeof i.parameters === "object")
          s.params = Object.entries(i.parameters as Record<string, unknown>).filter(([, v]) => num(v) !== undefined).map(([k, v]) => [k, v as number]);
        const r = await api.simulate(model, s);
        hooks.afterSimulate(model, r);
        const p = (await api.profile()) as unknown as { summary_text?: string } | null;
        return `${r.ok ? `simulation of ${model} finished.` : `simulation of ${model} FAILED: ${r.error}`}\n${p?.summary_text ?? "(no profile)"}`;
      },
    },
    {
      name: "flatten_model",
      description: "The flat model (all components expanded) as Modelica-like text, truncated to max_lines.",
      input_schema: { type: "object", properties: { model: { type: "string" }, max_lines: { type: "integer" } }, required: ["model"] },
      async run(i) {
        const t = await api.flatten(str(i.model) ?? "");
        const lines = t.split("\n");
        const max = num(i.max_lines) ?? 300;
        return lines.slice(0, max).join("\n") + (lines.length > max ? `\n… ${lines.length - max} more lines` : "");
      },
    },
    {
      name: "open_class",
      description: "Show a class in the workbench (diagram/text view) for the user.",
      input_schema: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
      async run(i) {
        await hooks.openClass(str(i.path) ?? "");
        return "opened";
      },
    },
  ];
}

const SYSTEM = `You are the assistant inside DAEdalus, an open-source Modelica modeling and simulation workbench that runs in the browser.
You help engineers build, debug and understand Modelica models with the tools provided: browse classes, read source, write user model files, check, simulate, and read analysis reports.
Rules:
- Library files (the Modelica Standard Library under msl/ and the bundled Daedalus library) are read-only. To change a model, write a user file (user/Name.mo) that defines a new class, extending or copying the original.
- After every edit, run check_model on the edited model and fix what it reports before telling the user it works.
- Ground claims in tool output: quote the numbers you got from a simulation or a check, and say when something is an inference.
- When a translation or simulation fails, explain the cause in engineering terms (which equation or component, why) and propose a concrete fix.
- Keep answers short and direct.`;

interface ChatEntry {
  role: "user" | "assistant" | "tool" | "error";
  text: string;
}

export function initAssistant(hooks: AssistantHooks) {
  const btn = document.createElement("button");
  btn.id = "btn-assistant";
  btn.textContent = "✦ Assistant";
  btn.title = "Ask Claude about the model";
  document.querySelector("#topbar .tools")!.appendChild(btn);

  const panel = document.createElement("aside");
  panel.id = "assistant";
  panel.className = "assistant-panel pane hidden";
  panel.innerHTML = `
    <div class="pane-title">Assistant <span class="muted">Claude</span><button class="close" title="Close">✕</button></div>
    <div class="assistant-starters">
      <p>Ask about the open model.</p>
      <button type="button" data-prompt="Why is this model stiff?">Why is this model stiff?</button>
      <button type="button" data-prompt="Add a damper between the masses.">Add a damper between the masses.</button>
      <button type="button" data-prompt="What does this model do?">What does this model do?</button>
    </div>
    <div class="assistant-key hidden">
      <p>The assistant calls the Claude API from this page with your own API key. The key is kept only in this browser.</p>
      <input type="password" placeholder="sk-ant-…" autocomplete="off" />
      <button type="button">Save key</button>
    </div>
    <div class="assistant-log"></div>
    <form class="assistant-input">
      <textarea rows="3" placeholder="Ask about the open model"></textarea>
      <div class="row"><button type="button" class="forget">Forget key</button><button type="button" class="stop hidden">Stop</button><button type="submit">Send</button></div>
    </form>`;
  const right = document.getElementById("right");
  const inspectorPane = document.getElementById("inspector-pane");
  if (right && inspectorPane) right.insertBefore(panel, inspectorPane);
  else document.body.appendChild(panel);
  const logEl = panel.querySelector<HTMLElement>(".assistant-log")!;
  const keyBox = panel.querySelector<HTMLElement>(".assistant-key")!;
  const keyInput = keyBox.querySelector<HTMLInputElement>("input")!;
  const form = panel.querySelector<HTMLFormElement>("form")!;
  const ta = form.querySelector<HTMLTextAreaElement>("textarea")!;
  const stopBtn = form.querySelector<HTMLButtonElement>(".stop")!;

  const getKey = (): string => {
    try {
      return localStorage.getItem(KEY_STORE) ?? "";
    } catch {
      return memoryKey;
    }
  };
  let memoryKey = "";
  const setKey = (k: string) => {
    memoryKey = k;
    try {
      if (k) localStorage.setItem(KEY_STORE, k);
      else localStorage.removeItem(KEY_STORE);
    } catch {
      /* private window: keep it in memory */
    }
    keyBox.classList.toggle("hidden", !!k);
  };
  let pending = "";
  const setOpen = (on: boolean) => {
    panel.classList.toggle("hidden", !on);
    inspectorPane?.classList.toggle("hidden", on);
    document.getElementById("app")?.classList.toggle("assistant-open", on);
    btn.classList.toggle("active", on);
  };
  keyBox.querySelector("button")!.addEventListener("click", () => {
    setKey(keyInput.value.trim());
    if ((getKey() || memoryKey) && pending) {
      const q = pending;
      pending = "";
      void send(q);
    }
  });
  form.querySelector(".forget")!.addEventListener("click", () => {
    setKey("");
    keyBox.classList.add("hidden");
  });
  panel.querySelectorAll<HTMLButtonElement>(".assistant-starters button").forEach((b) => {
    b.onclick = () => {
      const q = b.dataset.prompt ?? b.textContent ?? "";
      if (q) void send(q);
    };
  });

  btn.onclick = () => {
    const open = panel.classList.contains("hidden");
    setOpen(open);
    if (open) panel.querySelector<HTMLButtonElement>(".assistant-starters button")?.focus();
  };
  panel.querySelector(".close")!.addEventListener("click", () => setOpen(false));

  const show = (e: ChatEntry) => {
    const d = document.createElement(e.role === "tool" ? "details" : "div");
    d.className = `msg ${e.role}`;
    if (e.role === "tool") {
      const [head, ...rest] = e.text.split("\n");
      d.innerHTML = `<summary></summary><pre></pre>`;
      d.querySelector("summary")!.textContent = head;
      d.querySelector("pre")!.textContent = rest.join("\n");
    } else d.textContent = e.text;
    logEl.appendChild(d);
    logEl.scrollTop = logEl.scrollHeight;
    return d;
  };

  let messages: BetaMessageParam[] = [];
  let abort: AbortController | null = null;

  async function send(question: string) {
    const key = getKey() || memoryKey;
    if (!key) {
      pending = question;
      keyBox.classList.remove("hidden");
      keyInput.focus();
      return;
    }
    pending = "";
    const tools = [...builtinTools(hooks), ...extraTools];
    const byName = new Map(tools.map((t) => [t.name, t]));
    // Loaded on first use, so the SDK stays out of the page's initial bundle.
    const { default: Anthropic } = await import("@anthropic-ai/sdk");
    const client = new Anthropic({ apiKey: key, dangerouslyAllowBrowser: true });
    const cur = hooks.currentClass();
    show({ role: "user", text: question });
    const base = messages.length;
    messages.push({ role: "user", content: cur ? `(The workbench has ${cur} open.)\n\n${question}` : question });
    const busy = show({ role: "assistant", text: "…" });
    busy.classList.add("busy");
    abort = new AbortController();
    stopBtn.classList.remove("hidden");
    try {
      for (let turn = 0; turn < 25; turn++) {
        const res = await client.beta.messages.create(
          {
            model: MODEL,
            max_tokens: 16000,
            system: SYSTEM,
            thinking: { type: "adaptive" },
            tools: tools.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) as BetaToolUnion[],
            messages,
            betas: ["server-side-fallback-2026-07-01"],
            fallbacks: "default",
          },
          { signal: abort.signal },
        );
        messages.push({ role: "assistant", content: res.content });
        for (const b of res.content) if (b.type === "text" && b.text.trim()) logEl.insertBefore(show({ role: "assistant", text: b.text }), busy);
        if (res.stop_reason === "refusal") {
          logEl.insertBefore(show({ role: "error", text: "Claude declined this request." }), busy);
          break;
        }
        if (res.stop_reason === "pause_turn") continue;
        if (res.stop_reason !== "tool_use") break;
        const results: BetaToolResultBlockParam[] = [];
        for (const b of res.content) {
          if (b.type !== "tool_use") continue;
          const tool = byName.get(b.name);
          let text: string;
          let isError = false;
          try {
            if (!tool) throw new Error(`unknown tool ${b.name}`);
            text = await tool.run((b.input ?? {}) as Record<string, unknown>);
          } catch (e) {
            text = `error: ${e instanceof Error ? e.message : String(e)}`;
            isError = true;
          }
          const arg = JSON.stringify(b.input);
          logEl.insertBefore(show({ role: "tool", text: `${isError ? "✗" : "✓"} ${b.name} ${arg.length > 120 ? arg.slice(0, 117) + "…" : arg}\n${text}` }), busy);
          results.push({ type: "tool_result", tool_use_id: b.id, content: text.slice(0, 60000), is_error: isError });
        }
        messages.push({ role: "user", content: results });
      }
    } catch (e) {
      if (!(e instanceof Anthropic.APIUserAbortError)) {
        const msg = e instanceof Anthropic.AuthenticationError ? "The API key was rejected. Enter a valid key." : e instanceof Error ? e.message : String(e);
        logEl.insertBefore(show({ role: "error", text: msg }), busy);
        if (e instanceof Anthropic.AuthenticationError) setKey("");
        // Drop the failed exchange so the conversation stays valid.
        messages.length = base;
      }
    } finally {
      busy.remove();
      stopBtn.classList.add("hidden");
      abort = null;
    }
  }

  stopBtn.onclick = () => abort?.abort();
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const q = ta.value.trim();
    if (!q || abort) return;
    ta.value = "";
    send(q);
  });
  ta.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      form.requestSubmit();
    }
  });
  return { reset: () => ((messages = []), (logEl.innerHTML = "")) };
}
