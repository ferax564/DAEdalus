// Import parameter data (CSV `name,value[,unit]` or SSP `.ssv`) for a component: the
// values are checked against its type (unknown names, units converted), shown as a
// modifier and a record, and can be written into the model's source.

import { api } from "./rpc";
import type { ComponentInfo, ParamValue } from "./types";

export interface ParamsHost {
  /** Apply edit ops to the open class through the diagram editor (undoable); false on failure. */
  applyOps(ops: unknown[]): Promise<boolean>;
  log(msg: string, cls?: string): void;
}

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

function pick(): Promise<File | null> {
  return new Promise((resolve) => {
    const i = document.createElement("input");
    i.type = "file";
    i.accept = ".csv,.tsv,.txt,.ssv,.xml";
    i.onchange = () => resolve(i.files?.[0] ?? null);
    i.addEventListener("cancel", () => resolve(null));
    i.click();
  });
}

/** Pick a parameter file for component `comp` of model `model` and show the result. */
export async function importParams(model: string, comp: ComponentInfo, host: ParamsHost, file?: File) {
  const f = file ?? (await pick());
  if (!f) return;
  let params: ParamValue[];
  try {
    params = await api.paramsParse(await f.text(), f.name);
  } catch (e) {
    host.log(`${f.name}: ${e instanceof Error ? e.message : e}`, "err");
    return;
  }
  const recordName = `${comp.name[0].toUpperCase()}${comp.name.slice(1)}Data`;
  const r = await api.paramsFor(model, comp.name, params, recordName);
  const own = comp.declared_in === model;
  const dlg = document.createElement("dialog");
  dlg.className = "params-dialog";
  dlg.innerHTML = `<form method="dialog" class="dlg-head"><h3>Parameters for ${esc(comp.name)} from ${esc(f.name)}</h3><button value="close" title="Close">×</button></form>
    <table class="params-table"><thead><tr><th>Name</th><th>Value</th><th>Unit</th></tr></thead><tbody>${r.params
      .map((p) => `<tr><td>${esc(p.name)}</td><td><code>${esc(p.value)}</code></td><td>${esc(p.unit ?? "")}</td></tr>`)
      .join("")}</tbody></table>
    ${r.messages.length ? `<ul class="params-msgs">${r.messages.map((m) => `<li class="${m.startsWith("warning") ? "warn" : "muted"}">${esc(m)}</li>`).join("")}</ul>` : ""}
    <h4>Modifier</h4><pre class="params-code">${esc(r.modifier)}</pre>
    <h4>Record</h4><pre class="params-code">${esc(r.record)}</pre>
    <div class="dlg-actions">
      <button class="p-copy-mod">Copy modifier</button>
      <button class="p-copy-rec">Copy record</button>
      <button class="p-apply primary" ${own ? "" : "disabled"} title="${own ? `Merge the values into the declaration of ${esc(comp.name)}` : `${esc(comp.name)} is inherited from ${esc(comp.declared_in)}`}">Apply to ${esc(comp.name)}</button>
    </div>`;
  document.body.appendChild(dlg);
  dlg.addEventListener("close", () => dlg.remove());
  const copy = (t: string, what: string) =>
    navigator.clipboard?.writeText(t).then(
      () => host.log(`${what} copied`, "ok"),
      () => host.log(`clipboard unavailable`, "warn"),
    );
  dlg.querySelector<HTMLButtonElement>(".p-copy-mod")!.onclick = () => copy(r.modifier, "Modifier");
  dlg.querySelector<HTMLButtonElement>(".p-copy-rec")!.onclick = () => copy(r.record, "Record");
  dlg.querySelector<HTMLButtonElement>(".p-apply")!.onclick = async () => {
    try {
      // the diagram editor's set_modifier op: same writer as the inspector, with undo
      const ops = r.params.map((p) => ({ op: "set_modifier", name: comp.name, modifier: p.name, value: p.value }));
      if (!(await host.applyOps(ops))) return;
      host.log(`${r.params.length} parameters of ${comp.name} set from ${f.name}`, "ok");
      dlg.close();
    } catch (err) {
      host.log(String(err), "err");
    }
  };
  dlg.showModal();
}
