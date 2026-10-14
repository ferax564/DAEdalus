// The bundled Daedalus library, embedded at build time.
const files = import.meta.glob("../../library/Daedalus/**/*.mo", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

export function bundledLibrary(): [string, string][] {
  return Object.entries(files).map(([path, text]) => [path.replace(/^(\.\.\/)+library\//, "library/"), text]);
}

/** Read .mo files picked with a directory or file input. */
export async function readPicked(list: FileList): Promise<[string, string][]> {
  const out: [string, string][] = [];
  for (const f of Array.from(list)) {
    if (!f.name.endsWith(".mo")) continue;
    const rel = (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name;
    out.push([`user/${rel}`, await f.text()]);
  }
  return out;
}
