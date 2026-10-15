// The bundled Daedalus library, embedded at build time.
const files = import.meta.glob("../../library/Daedalus/**/*.mo", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

export function bundledLibrary(): [string, string][] {
  return Object.entries(files).map(([path, text]) => [path.replace(/^(\.\.\/)+library\//, "library/"), text]);
}

// Shape meshes of the bundled library (Daedalus.Visualizers), served as separate assets.
const meshUrls = import.meta.glob("../../library/Daedalus/**/*.{stl,obj}", { query: "?url", import: "default", eager: true }) as Record<string, string>;

/** Fetch the bundled library's shape meshes, keyed like `bundledLibrary` paths. */
export async function bundledResources(): Promise<[string, ArrayBuffer][]> {
  const out: [string, ArrayBuffer][] = [];
  await Promise.all(
    Object.entries(meshUrls).map(async ([path, url]) => {
      try {
        const r = await fetch(url);
        if (r.ok) out.push([path.replace(/^(\.\.\/)+library\//, "library/"), await r.arrayBuffer()]);
      } catch {
        // the shape falls back to a box
      }
    }),
  );
  return out;
}

/** Files picked with a directory or file input: Modelica sources, and resources such as the
 *  STL and OBJ files of MultiBody shapes, keyed by their relative path. */
export interface Picked {
  mo: [string, string][];
  resources: [string, ArrayBuffer][];
  /** Model packages and library archives. */
  zips: File[];
}

/** Resource files kept with a library: shape meshes, documentation images, data tables. */
const RESOURCE = /\.(stl|obj|png|jpe?g|gif|svg|webp|bmp|txt|csv|mat|json|xml|html?|pdf)$/i;

/** Read .mo files (and shape files) picked with a directory or file input. */
export async function readPicked(list: FileList): Promise<Picked> {
  const out: Picked = { mo: [], resources: [], zips: [] };
  for (const f of Array.from(list)) {
    const rel = `user/${(f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name}`;
    if (f.name.endsWith(".mo")) out.mo.push([rel, await f.text()]);
    else if (/\.zip$/i.test(f.name)) out.zips.push(f);
    else if (RESOURCE.test(f.name)) out.resources.push([rel, await f.arrayBuffer()]);
  }
  return out;
}
