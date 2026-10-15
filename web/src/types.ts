// Data shapes returned by the WebAssembly workspace (see crates/daedalus-wasm).

export interface ClassEntry {
  name: string;
  path: string;
  kind: string;
  partial: boolean;
  description: string;
  runnable: boolean;
  children: ClassEntry[];
}

/** Annotation value: records are objects tagged with "@". */
export type AnnValue = number | string | boolean | null | AnnValue[] | AnnRecord;
export interface AnnRecord {
  "@"?: string;
  [key: string]: AnnValue | undefined;
}

export interface Layer {
  coordinate_system: AnnRecord | null;
  graphics: AnnRecord[];
}

export interface ComponentInfo {
  name: string;
  type_name: string;
  type_path: string | null;
  type_kind: string;
  description: string;
  causality: string;
  variability: string;
  flow: boolean;
  protected: boolean;
  conditional: boolean;
  /** Condition of a conditional component as source text (`useHeatPort`). */
  condition: string | null;
  placement: AnnRecord | null;
  modifiers: [string, string][];
  binding: string | null;
  declared_in: string;
}

export interface ConnectionInfo {
  from: string;
  to: string;
  line: AnnRecord | null;
  declared_in: string;
}

export interface ClassInfo {
  path: string;
  name: string;
  kind: string;
  partial: boolean;
  description: string;
  source: { uri: string; start: number; end: number; line: number } | null;
  extends: string[];
  icon: Layer;
  diagram: Layer;
  components: ComponentInfo[];
  connections: ConnectionInfo[];
  documentation: string | null;
  experiment: AnnRecord | null;
}

export interface VarSummary {
  name: string;
  unit: string;
  description: string;
  kind: "series" | "param" | "alias";
  value: number | null;
  /** Parameter whose override changes the result (public, with a literal value). */
  editable: boolean;
}

export interface SimSummary {
  ok: boolean;
  error: string | null;
  model: string;
  vars: VarSummary[];
  points: number;
  log: string[];
  warnings: string[];
  translate_ms: number;
  /** Time spent parsing the library files the model needs (on first use). */
  load_ms?: number;
  stats: {
    solver: { steps: number; rejected: number; newton_failures: number; rhs_evals: number; jac_evals: number };
    events: number;
    states: number;
    init_ms: number;
    sim_ms: number;
  } | null;
  model_stats: Record<string, number> | null;
  experiment: Record<string, unknown> | null;
}

export interface SimSettings {
  start_time?: number;
  stop_time?: number;
  intervals?: number;
  interval?: number;
  tolerance?: number;
  method?: string;
  params?: [string, number][];
}

/** Progress of loading a library bundle in the worker. */
export interface BundleProgress {
  phase: "download" | "unpack" | "register";
  done: number;
  total: number;
}

/** A library bundle registered in the worker (see scripts/fetch-msl.mjs). */
export interface BundleInfo {
  name: string;
  version: string;
  files: number;
  bytes: number;
  download_ms: number;
  unpack_ms: number;
  register_ms: number;
}
