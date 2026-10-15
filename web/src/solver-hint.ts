// Solver choice: the grouped method list's tooltips and a "Recommend" hint read from the
// statistics of the last run (a stiff signature, many rejected steps).

/** Solver statistics of a run (see `SimStats` in daedalus-core). */
export interface RunStats {
  method?: string;
  states?: number;
  solver: {
    steps: number;
    rejected: number;
    newton_failures: number;
    rhs_evals: number;
    jac_evals: number;
    stiff_steps?: number;
    switches?: number;
  };
}

/** One-line descriptions, also used as the options' tooltips. */
export const METHOD_INFO: Record<string, string> = {
  "": "Use the experiment annotation's __Dymola_Algorithm, otherwise BDF",
  Bdf: "Stiff: variable-order BDF 1-5 (like Dassl/CVODE); robust default for most models",
  Radau: "Stiff: Radau IIA order 5, L-stable one-step; strong on very stiff or oscillating problems at tight tolerances",
  Esdirk: "Stiff: ESDIRK4(3)6L L-stable one-step; cheap restarts, good with frequent events",
  Auto: "Switching: starts with Dopri5, moves to BDF when steps become stability-limited, and back (like Lsodar)",
  Dopri5: "Non-stiff: explicit Dormand-Prince 5(4); fast when the model is not stiff",
  Rk4: "Fixed step: classic Runge-Kutta 4 with the output interval (or --step) as step size",
  Euler: "Fixed step: explicit Euler, for real-time style runs",
};

const NAMES: Record<string, string> = {
  bdf: "Bdf",
  radau: "Radau",
  esdirk: "Esdirk",
  auto: "Auto",
  dopri5: "Dopri5",
  rk4: "Rk4",
  euler: "Euler",
};

export interface Recommendation {
  /** Method to switch to (a `Method` value), if any. */
  method: string | null;
  text: string;
}

/** Recommend a method from a run's statistics, or null when the choice looks right. */
export function recommend(st: RunStats): Recommendation | null {
  const s = st.solver;
  const used = NAMES[(st.method ?? "bdf").toLowerCase()] ?? "Bdf";
  const steps = Math.max(s.steps, 1);
  const rejRate = s.rejected / (s.steps + s.rejected || 1);
  const pct = (x: number) => `${Math.round(100 * x)}%`;
  if (used === "Dopri5" || used === "Rk4" || used === "Euler") {
    const stiff = (s.stiff_steps ?? 0) / steps;
    if (used === "Dopri5" && s.steps >= 50 && stiff > 0.2)
      return {
        method: "Bdf",
        text: `Stiff: ${pct(stiff)} of the explicit steps were limited by stability. Try BDF or Radau (or Auto).`,
      };
    if (s.steps >= 50 && rejRate > 0.3)
      return { method: "Bdf", text: `${pct(rejRate)} of the steps were rejected. A stiff solver (BDF, Radau) may be faster.` };
    return null;
  }
  if (used === "Auto") {
    if ((s.switches ?? 0) > 20)
      return { method: "Bdf", text: `Auto switched methods ${s.switches} times. BDF alone avoids the restarts.` };
    return null;
  }
  // Implicit methods.
  if (s.steps >= 50 && s.newton_failures > 0.2 * steps)
    return {
      method: used === "Radau" ? "Bdf" : "Radau",
      text: `Newton failed on ${pct(s.newton_failures / steps)} of the steps. Try ${used === "Radau" ? "BDF" : "Radau"} or a tighter tolerance.`,
    };
  if (s.steps >= 100 && rejRate > 0.4)
    return {
      method: used === "Bdf" ? "Radau" : "Bdf",
      text: `${pct(rejRate)} of the steps were rejected. Try ${used === "Bdf" ? "Radau" : "BDF"}.`,
    };
  return null;
}

/** Put the tooltips on the method select's options. */
export function decorateMethodSelect(sel: HTMLSelectElement) {
  for (const o of Array.from(sel.options)) {
    const info = METHOD_INFO[o.value];
    if (info) o.title = info;
  }
  const sync = () => (sel.title = METHOD_INFO[sel.value] ?? "");
  sel.addEventListener("change", sync);
  sync();
}

/** Show (or hide) the recommendation chip next to the method select after a run. */
export function showRecommendation(chip: HTMLElement, sel: HTMLSelectElement, st: RunStats | null, log: (m: string, cls?: string) => void) {
  const r = st ? recommend(st) : null;
  chip.classList.toggle("hidden", !r);
  if (!r) return;
  chip.textContent = r.method ? `Recommend: ${sel.querySelector<HTMLOptionElement>(`option[value="${r.method}"]`)?.text ?? r.method}` : "Solver hint";
  chip.title = r.text + (r.method ? " Click to select it for the next run." : "");
  chip.onclick = () => {
    if (r.method) {
      sel.value = r.method;
      sel.dispatchEvent(new Event("change"));
    }
    chip.classList.add("hidden");
  };
  log(`solver hint: ${r.text}`, "warn");
}
