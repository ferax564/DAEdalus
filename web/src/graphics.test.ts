import { expect, test } from "vitest";
import { diagramInk, isNearBlackInk } from "./graphics";

test("near-black default ink follows the sheet and explicit colours stay", () => {
  expect(isNearBlackInk([0, 0, 0])).toBe(true);
  expect(isNearBlackInk([32, 32, 32])).toBe(true);
  expect(isNearBlackInk([20, 24, 28])).toBe(true);
  expect(isNearBlackInk([0, 0, 127])).toBe(false);
  expect(isNearBlackInk([255, 0, 0])).toBe(false);
  expect(isNearBlackInk([95, 95, 95])).toBe(false);
  expect(isNearBlackInk([0, 128, 255])).toBe(false);

  expect(diagramInk(undefined)).toBe("var(--canvas-ink)");
  expect(diagramInk([0, 0, 0])).toBe("var(--canvas-ink)");
  expect(diagramInk([20, 24, 28])).toBe("var(--canvas-ink)");
  expect(diagramInk([0, 0, 127])).toBe("rgb(0,0,127)");
  expect(diagramInk([0, 128, 255])).toBe("rgb(0,128,255)");
  expect(diagramInk([255, 255, 255])).toBe("rgb(255,255,255)");
  expect(diagramInk([95, 95, 95])).toBe("rgb(95,95,95)");
});
