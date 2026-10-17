// OpenRouter chat client for the in-app assistant. The user's key stays in the
// browser; each request goes from the page to OpenRouter's OpenAI-compatible API.

export const OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions";

/** GPT-6 Luna models currently published on OpenRouter.
 *  The `:batch` ids are batch-API variants; chat completions rejects them.
 */
export const LUNA_MODELS: { id: string; label: string }[] = [
  { id: "openai/gpt-6-luna", label: "GPT-6 Luna" },
  { id: "openai/gpt-6-luna-pro", label: "GPT-6 Luna Pro" },
  { id: "openai/gpt-6-luna:batch", label: "GPT-6 Luna (batch)" },
  { id: "openai/gpt-6-luna-pro:batch", label: "GPT-6 Luna Pro (batch)" },
];

export const DEFAULT_MODEL = LUNA_MODELS[0].id;

/** Pages published by web/scripts/build-docs.mjs, in the same order as the docs sidebar. */
export const DOC_PAGES: { id: string; title: string }[] = [
  { id: "index", title: "Overview" },
  { id: "getting-started", title: "Getting started" },
  { id: "whats-new", title: "What's new" },
  { id: "workbench", title: "Workbench" },
  { id: "editor", title: "Text editor" },
  { id: "plotting", title: "Live plots and dashboards" },
  { id: "animation", title: "3-D animation" },
  { id: "experiments", title: "Experiments and sweeps" },
  { id: "libraries", title: "Libraries and packages" },
  { id: "cli", title: "Command line" },
  { id: "scripting", title: "Scripts (.mos) and steady states" },
  { id: "python", title: "Python API" },
  { id: "assistant", title: "AI assistant and MCP" },
  { id: "examples", title: "Example library" },
  { id: "multibody", title: "Multibody vehicles" },
  { id: "msl", title: "Modelica Standard Library" },
  { id: "state-machines", title: "State machines" },
  { id: "structure", title: "Structure and state selection" },
  { id: "debugger", title: "Equation and init debugger" },
  { id: "units", title: "Unit checking" },
  { id: "performance", title: "Performance analyzer" },
  { id: "solvers", title: "Solvers" },
  { id: "linearize", title: "Linearization" },
  { id: "linear-analysis", title: "Linear analysis" },
  { id: "analysis", title: "Sensitivity, DOE and calibration" },
  { id: "optimal-control", title: "Optimal control, MPC, MHE" },
  { id: "sciml", title: "Scientific machine learning" },
  { id: "fmi", title: "FMI and SSP" },
  { id: "embedded", title: "Embedded C and processor in the loop" },
  { id: "architecture", title: "Architecture" },
  { id: "roadmap", title: "Roadmap" },
];

export const SYSTEM = `You are the assistant inside DAEdalus, a free open-source Modelica workbench for modeling physical systems (engines, circuits, vehicles, hydraulics, control loops). It runs in the browser and on the command line. The language is Modelica: models are built from components and the connections between them.

The workbench has three modes:
- Modeling: the package browser, and the open class as an icon, a diagram (components, connections and graphics), text, the class's own documentation, the flat model, the structure report, linearization and a diff. Library classes are read-only. The user's own models live in user files and can be edited on the diagram.
- Simulation: Check translates the model. Simulate runs it. The variable browser, live plots (including parametric plots) and, for models with shapes, the 3-D animation show the result.
- Analysis: sensitivities, Monte Carlo and Sobol uncertainty, design of experiments and calibration.

Libraries loaded in the workbench are the bundled Daedalus example library, the Modelica Standard Library, and any library the user opens.

The engine stays on the machine. In the browser it is the same compiler and solvers as the command line, compiled to WebAssembly and run in a worker. \`daedalus serve\` is that engine on this computer instead. Checks, simulations, plots and analysis do not leave the machine. This chat is the exception: each turn is sent to OpenRouter (https://openrouter.ai/api/v1/chat/completions), which forwards it to the provider of the selected model. You send the text you read (source, documentation pages, tool output) and the numbers you ask for. Do not invent figures.

You have the whole DAEdalus manual through read_documentation: every page the Docs site serves, not only the class that is open. For a question about DAEdalus, the workbench, a feature, a library or an example, read the relevant page before you answer. The open class is the model the user is looking at; it does not replace the manual.

You also have tools to browse classes, read source, write user model files, check, simulate and read analysis reports.

Rules:
- Library files (the Modelica Standard Library under msl/ and the bundled Daedalus library) are read-only. To change a model, write a user file (user/Name.mo) that defines a new class, extending or copying the original.
- After every edit, run check_model on the edited model and fix what it reports before telling the user it works.
- Ground claims in tool output. Quote numbers from a simulation, a check or a documentation page, and say when something is an inference.
- When a translation or simulation fails, explain the cause in engineering terms (which equation or component, why) and propose a concrete fix.
- Keep answers short and direct.`;

export interface ORToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

/** One OpenAI-style chat message. Assistant turns may carry tool calls. */
export interface ORMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string | null;
  tool_calls?: ORToolCall[];
  tool_call_id?: string;
  reasoning?: string;
  reasoning_details?: unknown;
}

export interface ORTool {
  name: string;
  description: string;
  parameters: { type: "object"; properties: Record<string, unknown>; required?: string[] };
}

export class OpenRouterError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "OpenRouterError";
    this.status = status;
  }
}

export function isAbortError(e: unknown): boolean {
  return e instanceof Error && e.name === "AbortError";
}

export function isAuthError(e: unknown): boolean {
  return e instanceof OpenRouterError && e.status === 401;
}

const NAMED_ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  ndash: "–",
  mdash: "—",
  hellip: "…",
};

function decodeEntities(s: string): string {
  return s.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (all, body: string) => {
    if (body[0] === "#") {
      const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : all;
    }
    return NAMED_ENTITIES[body] ?? all;
  });
}

/** Visible text of a built documentation page (the `<article>` the docs site serves). */
export function articleText(html: string): string {
  const article = html.match(/<article>([\s\S]*?)<\/article>/i)?.[1] ?? "";
  const withBreaks = article
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<style[\s\S]*?<\/style>/gi, "")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(p|div|h[1-6]|tr|li|pre|table|blockquote|figcaption)>/gi, "\n")
    .replace(/<li[^>]*>/gi, "- ")
    .replace(/<h([1-6])[^>]*>/gi, (_, n) => `${"#".repeat(Number(n))} `)
    .replace(/<[^>]+>/g, "");
  return decodeEntities(withBreaks)
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .replace(/[ \t]{2,}/g, " ")
    .trim();
}

/** Drop credential-shaped strings before text is sent to the provider. */
export function redactSecrets(text: string): string {
  return text.replace(/\bsk-or-v1-[A-Za-z0-9_-]+\b/g, "[redacted]").replace(/\bsk-ant-[A-Za-z0-9_-]+\b/g, "[redacted]");
}

function toolArgs(raw: unknown): string {
  if (typeof raw === "string") return raw;
  if (raw && typeof raw === "object") return JSON.stringify(raw);
  return "{}";
}

export function parseToolArguments(raw: string): Record<string, unknown> {
  if (!raw.trim()) return {};
  try {
    const v = JSON.parse(raw) as unknown;
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

interface ORChoice {
  finish_reason?: string;
  message?: {
    content?: string | null;
    tool_calls?: { id?: string; type?: string; function?: { name?: string; arguments?: unknown } }[];
    reasoning?: string;
    reasoning_details?: unknown;
    refusal?: string | null;
  };
}

/** One chat completion. `messages` is the conversation without the system prompt. */
export async function openRouterChat(opts: {
  apiKey: string;
  model: string;
  messages: ORMessage[];
  tools: ORTool[];
  signal?: AbortSignal;
}): Promise<{ message: ORMessage; finishReason: string }> {
  const res = await fetch(OPENROUTER_URL, {
    method: "POST",
    signal: opts.signal,
    headers: {
      Authorization: `Bearer ${opts.apiKey}`,
      "Content-Type": "application/json",
      "HTTP-Referer": "https://github.com/ferax564/DAEdalus",
      "X-Title": "DAEdalus",
    },
    body: JSON.stringify({
      model: opts.model,
      max_tokens: 8000,
      reasoning: { effort: "low" },
      messages: [{ role: "system", content: SYSTEM }, ...opts.messages],
      tools: opts.tools.map((t) => ({
        type: "function",
        function: { name: t.name, description: t.description, parameters: t.parameters },
      })),
    }),
  });
  const raw = await res.text();
  let data: { error?: { message?: string }; choices?: ORChoice[] } | null = null;
  try {
    data = JSON.parse(raw) as { error?: { message?: string }; choices?: ORChoice[] };
  } catch {
    data = null;
  }
  if (!res.ok) {
    const message = data?.error?.message || raw.slice(0, 500) || res.statusText;
    throw new OpenRouterError(res.status, message);
  }
  const choice = data?.choices?.[0];
  const msg = choice?.message;
  if (!msg) throw new OpenRouterError(res.status, "OpenRouter returned no message");
  const content = typeof msg.content === "string" ? msg.content : "";
  const out: ORMessage = {
    role: "assistant",
    content: content.trim() ? content : null,
  };
  if (Array.isArray(msg.tool_calls) && msg.tool_calls.length) {
    out.tool_calls = msg.tool_calls
      .filter((c) => c.function?.name)
      .map((c, i) => ({
        id: c.id || `call_${i}`,
        type: "function" as const,
        function: { name: c.function?.name ?? "", arguments: toolArgs(c.function?.arguments) },
      }));
    if (!out.tool_calls.length) delete out.tool_calls;
  }
  if (typeof msg.reasoning === "string" && msg.reasoning) out.reasoning = msg.reasoning;
  if (msg.reasoning_details != null) out.reasoning_details = msg.reasoning_details;
  if (msg.refusal) out.content = out.content ? `${out.content}\n${msg.refusal}` : msg.refusal;
  return { message: out, finishReason: choice?.finish_reason ?? (out.tool_calls?.length ? "tool_calls" : "stop") };
}
