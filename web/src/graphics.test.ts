import { expect, test } from "vitest";
import { IDENTITY, SVG_NS, diagramInk, drawGraphics, isLightFill, isNearBlackInk, newCtx } from "./graphics";

test("near-black default ink follows the sheet and explicit colours stay", () => {
  expect(isNearBlackInk([0, 0, 0])).toBe(true);
  expect(isNearBlackInk([32, 32, 32])).toBe(true);
  expect(isNearBlackInk([20, 24, 28])).toBe(true);
  expect(isNearBlackInk([0, 0, 127])).toBe(false);
  expect(isNearBlackInk([255, 0, 0])).toBe(false);
  expect(isNearBlackInk([95, 95, 95])).toBe(false);
  expect(isNearBlackInk([0, 128, 255])).toBe(false);

  expect(isLightFill([255, 255, 255])).toBe(true);
  expect(isLightFill([250, 250, 250])).toBe(true);
  expect(isLightFill([95, 95, 95])).toBe(false);
  expect(isLightFill([0, 0, 0])).toBe(false);

  expect(diagramInk(undefined)).toBe("var(--canvas-ink)");
  expect(diagramInk([0, 0, 0])).toBe("var(--canvas-ink)");
  expect(diagramInk([20, 24, 28])).toBe("var(--canvas-ink)");
  expect(diagramInk([0, 0, 0], true)).toBe("rgb(0,0,0)");
  expect(diagramInk(undefined, true)).toBe("rgb(0,0,0)");
  expect(diagramInk([20, 24, 28], true)).toBe("rgb(20,24,28)");
  expect(diagramInk([0, 0, 127])).toBe("rgb(0,0,127)");
  expect(diagramInk([0, 0, 127], true)).toBe("rgb(0,0,127)");
  expect(diagramInk([0, 128, 255])).toBe("rgb(0,128,255)");
  expect(diagramInk([255, 255, 255])).toBe("rgb(255,255,255)");
  expect(diagramInk([95, 95, 95])).toBe("rgb(95,95,95)");
});

test("near-black ink stays dark on a light fill and follows the sheet outside it", () => {
  const svg = document.createElementNS(SVG_NS, "svg");
  const g = document.createElementNS(SVG_NS, "g");
  svg.appendChild(g);
  drawGraphics(
    g,
    [
      { "@": "Rectangle", extent: [[-100, -100], [100, 100]], lineColor: [0, 0, 0], fillColor: [255, 255, 255], fillPattern: "FillPattern.Solid" },
      { "@": "Text", extent: [[-90, 0], [90, -40]], textString: "ME", textColor: [0, 0, 0] },
      { "@": "Line", points: [[-40, 10], [40, 10]], color: [0, 0, 0] },
      { "@": "Text", extent: [[-40, 140], [40, 110]], textString: "out", textColor: [0, 0, 0] },
      { "@": "Line", points: [[-40, 160], [40, 160]], color: [0, 0, 0] },
      { "@": "Text", extent: [[-40, 40], [40, 0]], textString: "blue", textColor: [0, 0, 127] },
    ],
    IDENTITY,
    newCtx(svg),
  );
  const rect = g.querySelector("rect");
  expect(rect?.getAttribute("fill")).toBe("rgb(255,255,255)");
  expect(rect?.getAttribute("stroke")).toBe("rgb(0,0,0)");
  const texts = [...g.querySelectorAll("text")];
  expect(texts.map((t) => t.textContent)).toEqual(["ME", "out", "blue"]);
  expect(texts[0].getAttribute("fill")).toBe("rgb(0,0,0)");
  expect(texts[1].getAttribute("fill")).toBe("var(--canvas-ink)");
  expect(texts[2].getAttribute("fill")).toBe("rgb(0,0,127)");
  const lines = [...g.querySelectorAll("path")];
  expect(lines[0].getAttribute("stroke")).toBe("rgb(0,0,0)");
  expect(lines[1].getAttribute("stroke")).toBe("var(--canvas-ink)");
});
