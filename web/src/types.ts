import type { DisplayChoices } from "./plot";
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
  /** Declared unit, or the one the unit checker inferred (unit_inferred). */
  unit: string;
  /** The `displayUnit` attribute. */
  display_unit?: string;
  unit_inferred?: boolean;
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
    solver: {
      steps: number;
      rejected: number;
      newton_failures: number;
      rhs_evals: number;
      jac_evals: number;
      tangent_evals?: number;
      stiff_steps?: number;
      switches?: number;
    };
    method?: string;
    events: number;
    states: number;
    init_ms: number;
    sim_ms: number;
  } | null;
  model_stats: Record<string, number> | null;
  experiment: Record<string, unknown> | null;
  /** Display-unit choices keyed by "unit|displayUnit". */
  display?: Record<string, DisplayChoices>;
}

/** Unit issue found by the unit checker (crates/daedalus-core/src/units.rs). */
export interface UnitIssue {
  origin: string;
  equation: string;
  left: string;
  left_unit: string;
  right: string;
  right_unit: string;
  message: string;
}

export interface UnitSummary {
  issues: UnitIssue[];
  declared: number;
  inferred: number;
  unknown: number;
  unparsed: string[];
}

export interface SimSettings {
  start_time?: number;
  stop_time?: number;
  intervals?: number;
  interval?: number;
  tolerance?: number;
  method?: string;
  params?: [string, number][];
  /** stateSelect overrides `[name or glob, tier]` (Structure view). */
  stateOverrides?: [string, string][];
  /** Record a performance profile and event log (see performance.ts). */
  profile?: boolean;
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

/** Options of a linear analysis (see daedalus_core::lti::LinearOptions). */
export interface LinearOptions {
  at?: number;
  params?: [string, number][];
  inputs?: string[];
  outputs?: string[];
  w_min?: number;
  w_max?: number;
  n_freq?: number;
  t_final?: number;
  n_time?: number;
  bode?: boolean;
  step?: boolean;
  reduce?: number;
  lqr?: [number, number];
}

export type Complex = [number, number];

export interface BodeData {
  omega: number[];
  mag_db: number[];
  phase_deg: number[];
  re: number[];
  im: number[];
}

export interface TimeData {
  t: number[];
  y: number[];
}

export interface PidGains {
  method: string;
  kp: number;
  ti: number;
  td: number;
  ki: number;
  kd: number;
}

export interface LinearPair {
  input: string;
  output: string;
  dc_gain: number | null;
  relative_degree: number | null;
  hf_gain: number;
  zeros: Complex[];
  margins: { gain_margin_db: number | null; phase_crossover: number | null; phase_margin_deg: number | null; gain_crossover: number | null };
  bode: BodeData | null;
  step: TimeData | null;
  impulse: TimeData | null;
  step_info: { final_value: number | null; rise_time: number | null; settling_time: number | null; overshoot: number | null; peak: number; peak_time: number } | null;
  pid: { fopdt: { k: number; t: number; l: number }; simc: PidGains; tau_c: number; ziegler_nichols: PidGains | null; note: string | null } | null;
  reduced_bode: BodeData | null;
  reduced_step: TimeData | null;
}

export interface LinearAnalysis {
  model: string;
  time: number;
  states: string[];
  inputs: string[];
  outputs: string[];
  poles: Complex[];
  stable: boolean;
  pairs: LinearPair[];
  reduction: { order: number; hankel_singular_values: number[]; error_bound: number; poles: Complex[] } | null;
  reduction_error: string | null;
  lqr: { q: number; r: number; k: number[][]; closed_loop_poles: Complex[] } | null;
  lqr_error: string | null;
}

export interface LinearResult {
  ok: boolean;
  error?: string | null;
  analysis?: LinearAnalysis | null;
  inputs: string[];
  outputs: string[];
  states: string[];
  candidates: string[];
  translate_ms: number;
  analysis_ms: number;
}
