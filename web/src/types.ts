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
  /** `Documentation(revisions=...)`. */
  revisions: string | null;
  /** `__Daedalus_metadata(key="value", ...)` pairs. */
  metadata: [string, string][];
  /** `version` annotation (a library's top-level package). */
  version: string | null;
  /** `uses(Lib(version="..."))` pairs. */
  uses: [string, string][];
  experiment: AnnRecord | null;
}

/** One line of a text diff (see crates/daedalus-core/src/diff.rs). */
export interface DiffLine {
  op: " " | "+" | "-";
  old: number | null;
  new: number | null;
  text: string;
}

export interface ComponentChange {
  name: string;
  change: "added" | "removed" | "modified";
  old_type: string | null;
  new_type: string | null;
  parameter: boolean;
  moved: boolean;
  old_placement: AnnRecord | null;
  new_placement: AnnRecord | null;
  details: string[];
}

export interface ConnectionChange {
  from: string;
  to: string;
  change: "added" | "removed" | "rerouted";
  old_line: AnnRecord | null;
  new_line: AnnRecord | null;
}

/** Structural and text diff of one class between two versions. */
export interface ClassDiff {
  old_class: string;
  new_class: string;
  extends: [string, string][];
  components: ComponentChange[];
  connections: ConnectionChange[];
  equations: { change: "added" | "removed"; initial: boolean; text: string }[];
  other: string[];
  text: DiffLine[];
}

/** A parameter value imported from CSV or SSV. */
export interface ParamValue {
  name: string;
  value: string;
  unit: string | null;
}

/** A library available to `uses` resolution. */
export interface LibAvailable {
  name: string;
  version: string | null;
  origin: string;
  loaded: boolean;
}

export interface LibReport {
  loaded: LibAvailable[];
  requirements: { by: string; name: string; version: string }[];
  resolution: { selected: LibAvailable[]; missing: string[]; conflicts: string[]; notes: string[] };
}

/** The bundled index of open libraries (library/packages.json). */
export interface LibIndex {
  libraries: {
    name: string;
    description: string;
    repo: string;
    license: string;
    packages: string[];
    versions: { version: string; ref: string; uses?: Record<string, string> }[];
  }[];
}

export interface LibSource {
  name: string | null;
  version: string | null;
  repo: string;
  git_ref: string;
  packages: string[];
}

/** The manifest of a model package (daedalus-package.json). */
export interface PackageManifest {
  format: number;
  model: string;
  files: string[];
  resources: string[];
  uses: Record<string, string>;
  missing: string[];
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
