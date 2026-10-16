/** Stored under `daedalus.theme`. `system` follows the OS and leaves the attribute unset. */
export type ThemeChoice = "system" | "light" | "dark";

export function readThemeChoice(raw: string | null): ThemeChoice {
  return raw === "light" || raw === "dark" || raw === "system" ? raw : "system";
}

/** Apply a theme choice to `<html>` before or after paint. */
export function applyThemeChoice(root: HTMLElement, choice: ThemeChoice): void {
  if (choice === "light" || choice === "dark") {
    root.dataset.theme = choice;
    root.style.colorScheme = choice;
  } else {
    delete root.dataset.theme;
    root.style.colorScheme = "";
  }
}
