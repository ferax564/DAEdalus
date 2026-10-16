import { expect, test, vi } from "vitest";

vi.mock("./rpc", () => ({
  api: {
    parameters: vi.fn(async () => [
      {
        name: "R",
        type_name: "Real",
        unit: "Ohm",
        description: "resistance",
        default: "1",
        declared_in: "Resistor",
      },
    ]),
  },
}));

import { DiagramEditor } from "./diagram-edit";
import type { ComponentInfo } from "./types";

const comp: ComponentInfo = {
  name: "resistor",
  type_name: "Resistor",
  type_path: "Modelica.Electrical.Analog.Basic.Resistor",
  type_kind: "model",
  description: "ideal resistor",
  causality: "",
  variability: "",
  flow: false,
  protected: false,
  conditional: false,
  condition: null,
  placement: null,
  modifiers: [],
  binding: null,
  declared_in: "Circuit",
};

test("Escape closes the parameter dialog and restores focus", async () => {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  const editor = new DiagramEditor({ svg, onRendered: () => {} } as never, {
    current: () => null,
    view: () => "diagram",
    classInfo: async () => null,
    documentChanged: async () => {},
    log: () => {},
  }, document.body);
  const trigger = document.createElement("button");
  trigger.textContent = "component";
  document.body.appendChild(trigger);
  trigger.focus();
  await editor.paramDialog(comp);
  const dialog = document.querySelector("[role=dialog]");
  expect(dialog?.getAttribute("aria-modal")).toBe("true");
  expect(document.body.contains(dialog)).toBe(true);
  const input = document.querySelector<HTMLInputElement>("input");
  expect(input).toBeTruthy();
  input!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(document.querySelector("[role=dialog]")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
