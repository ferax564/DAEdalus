// 3-D animation of Modelica.Mechanics.MultiBody results, in the style of Dymola's animation window.
//
// Every MultiBody visual (Body cylinders and spheres, joint cylinders, springs, FixedShape, world
// axes) is an instance of Visualizers.Advanced.Shape. Its inputs r, R.T, r_shape, lengthDirection,
// widthDirection, length, width, height, color and extra are ordinary variables of the result, so
// the view needs no model-specific code: it finds each shape in the variable list, reads its
// trajectories and draws it. Only `shapeType` is a String, which the result does not carry; it is
// resolved from the class definitions by `resolveShapeTypes`. A shapeType naming an STL or OBJ
// file is drawn from the file's bytes, which the caller passes in `AnimData.meshes`.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { STLLoader } from "three/examples/jsm/loaders/STLLoader.js";
import { OBJLoader } from "three/examples/jsm/loaders/OBJLoader.js";
import { RoomEnvironment } from "three/examples/jsm/environments/RoomEnvironment.js";
import { mergeGeometries, mergeVertices, toCreasedNormals } from "three/examples/jsm/utils/BufferGeometryUtils.js";

/** A constant, or one value per output point. */
export type Track = number | Float64Array | number[];

export interface AnimShape {
  /** Instance name of the Shape, e.g. `car.FL.lowerArm.frameTranslation.shape`. */
  name: string;
  type: string;
  r: Track[];
  /** R.T row-major (3x3): transforms world vectors into the shape's frame. */
  T: Track[];
  r_shape: Track[];
  lengthDirection: Track[];
  widthDirection: Track[];
  length: Track;
  width: Track;
  height: Track;
  extra: Track;
  color: Track[];
  specularCoefficient: Track;
}

export interface AnimData {
  model: string;
  time: Float64Array | number[];
  /** world.n: gravity direction; the view puts "up" opposite to it. */
  gravity: number[];
  shapes: AnimShape[];
  /** Contents of external shape files (STL, OBJ), keyed by the exact shapeType URI;
   *  a string is the file in base64. */
  meshes?: Record<string, ArrayBuffer | string>;
}

/** Shape instance names found in a list of result variable names. */
export function findShapes(names: Iterable<string>): string[] {
  const set = new Set(names);
  const out: string[] = [];
  for (const n of set) {
    if (!n.endsWith(".lengthDirection[1]")) continue;
    const p = n.slice(0, -".lengthDirection[1]".length);
    if (set.has(`${p}.R.T[1,1]`) && set.has(`${p}.r_shape[1]`) && set.has(`${p}.length`) && set.has(`${p}.color[1]`)) out.push(p);
  }
  return out.sort();
}

/** Result variables needed for one shape, in the order `assembleShape` consumes them. */
export function shapeVarNames(p: string): string[] {
  const v3 = (f: string) => [1, 2, 3].map((i) => `${p}.${f}[${i}]`);
  return [
    ...v3("r"),
    ...[1, 2, 3].flatMap((i) => [1, 2, 3].map((j) => `${p}.R.T[${i},${j}]`)),
    ...v3("r_shape"),
    ...v3("lengthDirection"),
    ...v3("widthDirection"),
    `${p}.length`,
    `${p}.width`,
    `${p}.height`,
    `${p}.extra`,
    ...v3("color"),
    `${p}.specularCoefficient`,
  ];
}

/** Build a shape from its tracks, given in `shapeVarNames` order. */
export function assembleShape(name: string, type: string, t: Track[]): AnimShape {
  return {
    name,
    type,
    r: t.slice(0, 3),
    T: t.slice(3, 12),
    r_shape: t.slice(12, 15),
    lengthDirection: t.slice(15, 18),
    widthDirection: t.slice(18, 21),
    length: t[21],
    width: t[22],
    height: t[23],
    extra: t[24],
    color: t.slice(25, 28),
    specularCoefficient: t[28],
  };
}

/** Collapse a track to a number when it does not change (saves memory and per-frame work). */
export function compact(y: Float64Array | number[]): Track {
  if (!y.length) return 0;
  const y0 = y[0];
  for (let i = 1; i < y.length; i++) if (y[i] !== y0) return y;
  return y0;
}

// ------------------------------------------------------------------ shapeType resolution

/** Minimal view of the workspace class information used to resolve String parameters. */
export interface ClassSource {
  get(path: string): Promise<{
    components: { name: string; type_path: string | null; modifiers: [string, string][]; binding: string | null }[];
  } | null>;
}

const STR = /^\s*"((?:[^"\\]|\\.)*)"\s*$/;
const IDENT = /^\s*([A-Za-z_]\w*)\s*$/;

/** Look up `key` (e.g. `shapeType` or `shape.shapeType`) in a component's modifiers, including
 *  nested class modifications such as `shape(shapeType="box")`. */
function findModifier(mods: [string, string][], key: string): string | null {
  for (const [k, v] of mods) {
    if (k === key) return v;
    if (key.startsWith(k + ".")) {
      // v is the whole modification text: `k(a=..., b(c=...))`
      const inner = v.slice(v.indexOf("(") + 1, v.lastIndexOf(")"));
      const r = findModifier(splitArgs(inner), key.slice(k.length + 1));
      if (r !== null) return r;
    }
  }
  return null;
}

/** Split `a=1, b(c="x,y")=2, d="z"` into [name, value] pairs (nested modifications keep their text). */
function splitArgs(s: string): [string, string][] {
  const out: [string, string][] = [];
  let depth = 0, inStr = false, start = 0;
  const push = (part: string) => {
    part = part.trim().replace(/^(final|each|redeclare)\s+/, "");
    const m = /^([A-Za-z_][\w.]*)\s*(\(.*\))?\s*(?:=\s*([\s\S]*))?$/.exec(part);
    if (!m) return;
    if (m[2]) out.push([m[1], m[1] + m[2]]);
    if (m[3] !== undefined) out.push([m[1], m[3].trim()]);
  };
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inStr) {
      if (c === "\\") i++;
      else if (c === '"') inStr = false;
    } else if (c === '"') inStr = true;
    else if (c === "(" || c === "{" || c === "[") depth++;
    else if (c === ")" || c === "}" || c === "]") depth--;
    else if (c === "," && depth === 0) {
      push(s.slice(start, i));
      start = i + 1;
    }
  }
  push(s.slice(start));
  return out;
}

/**
 * Resolve the `shapeType` of each Shape instance by walking the instance path through the class
 * definitions: an outer modifier wins over an inner one, which wins over the declaration's
 * binding; a value that names a parameter (`shapeType=shapeType`) is resolved one level up.
 */
export async function resolveShapeTypes(model: string, shapes: string[], src: ClassSource): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  for (const s of shapes) {
    let t: string | null = null;
    try {
      t = await resolveString(model, s.split(/\.(?![^[]*\])/), "shapeType", src);
    } catch {
      t = null;
    }
    out.set(s, t ?? guessType(s));
  }
  return out;
}

interface Level {
  /** Class of this instance. */
  cls: string;
  /** Modifiers on the component declaration that created it (none for the root). */
  mods: [string, string][];
}

async function resolveString(model: string, path: string[], param: string, src: ClassSource, depth = 0): Promise<string | null> {
  if (depth > 12) return null;
  // levels[0] is the model itself; levels[i] the instance path[0..i-1]
  const levels: Level[] = [{ cls: model, mods: [] }];
  for (const seg of path) {
    const prev = levels[levels.length - 1];
    const info = await src.get(prev.cls);
    const name = seg.replace(/\[.*\]$/, "");
    const c = info?.components.find((k) => k.name === name);
    if (!c || !c.type_path) return null;
    levels.push({ cls: c.type_path, mods: c.modifiers });
  }
  const k = levels.length - 1;
  // outermost modifier first
  for (let i = 1; i <= k; i++) {
    const key = [...path.slice(i, k).map((s) => s.replace(/\[.*\]$/, "")), param].join(".");
    const v = findModifier(levels[i].mods, key);
    if (v !== null) return evalString(v, model, path.slice(0, i - 1), src, depth);
  }
  const info = await src.get(levels[k].cls);
  const decl = info?.components.find((c) => c.name === param);
  if (decl?.binding) return evalString(decl.binding, model, path, src, depth);
  return null;
}

async function evalString(expr: string, model: string, scope: string[], src: ClassSource, depth: number): Promise<string | null> {
  const s = STR.exec(expr);
  if (s) return s[1];
  const id = IDENT.exec(expr);
  if (id) return resolveString(model, scope, id[1], src, depth + 1);
  return null;
}

function guessType(name: string): string {
  const last = name.split(".").pop() ?? "";
  if (/sphere/i.test(last)) return "sphere";
  if (/spring/i.test(last)) return "spring";
  if (/box/i.test(last)) return "box";
  if (/head|cone/i.test(last)) return "cone";
  return "cylinder";
}

// ------------------------------------------------------------------ geometry

/** Unit geometries in the Shape frame: length along +x from 0 to 1, width along y and height
 *  along z, centred, both 1. */
const geomCache = new Map<string, THREE.BufferGeometry>();
function unitGeometry(type: string, extra: number, wireRatio: number): THREE.BufferGeometry {
  const key = type === "spring" ? `spring:${extra.toFixed(2)}:${wireRatio.toFixed(3)}` : type === "pipe" || type === "cone" ? `${type}:${extra.toFixed(3)}` : type;
  let g = geomCache.get(key);
  if (g) return g;
  const toX = new THREE.Matrix4().makeRotationZ(-Math.PI / 2); // cylinder axis y -> x
  switch (type) {
    case "sphere":
      g = new THREE.SphereGeometry(0.5, 48, 24).translate(0.5, 0, 0);
      break;
    case "cylinder":
    case "gearwheel":
      g = new THREE.CylinderGeometry(0.5, 0.5, 1, 48).applyMatrix4(toX).translate(0.5, 0, 0);
      break;
    case "cone": {
      const top = Math.max(0, Math.min(1, extra)) * 0.5;
      g = new THREE.CylinderGeometry(top, 0.5, 1, 48).applyMatrix4(toX).translate(0.5, 0, 0);
      break;
    }
    case "pipe": {
      const ri = Math.max(0.01, Math.min(0.99, extra || 0.5)) * 0.5;
      const prof = [new THREE.Vector2(ri, 0), new THREE.Vector2(0.5, 0), new THREE.Vector2(0.5, 1), new THREE.Vector2(ri, 1), new THREE.Vector2(ri, 0)];
      g = new THREE.LatheGeometry(prof, 48).translate(0, -0.5, 0).applyMatrix4(toX).translate(0.5, 0, 0);
      g.computeVertexNormals();
      break;
    }
    case "spring": {
      const turns = Math.max(1, extra || 5);
      const pts: THREE.Vector3[] = [];
      const n = Math.ceil(turns * 24);
      const rc = 0.5 - wireRatio / 2;
      for (let i = 0; i <= n; i++) {
        const a = (i / n) * turns * 2 * Math.PI;
        pts.push(new THREE.Vector3(i / n, rc * Math.cos(a), rc * Math.sin(a)));
      }
      g = new THREE.TubeGeometry(new THREE.CatmullRomCurve3(pts), n * 2, Math.max(0.01, wireRatio / 2), 8, false);
      break;
    }
    default: // box, beam, and file shapes we cannot load
      g = new THREE.BoxGeometry(1, 1, 1).translate(0.5, 0, 0);
  }
  geomCache.set(key, g);
  return g;
}

// ------------------------------------------------------------------ external meshes

/** Whether a shapeType names a file (`modelica://Lib/Resources/x.stl`, `file:///...`) rather than a primitive. */
export function isFileShape(type: string): boolean {
  return /^(modelica|file):\/\//i.test(type) || /\.(stl|obj)$/i.test(type);
}

/**
 * Find the loaded resource a shape URI refers to, among paths such as
 * `user/libs/MultiBodyVehicle/Resources/Shapes/tyre.stl`. `modelica://Pkg.Sub/Rest` matches a path
 * ending in `/Pkg/Sub/Rest` (or `/Pkg/Rest` when Sub is a file); `file:///abs/path` matches a
 * path whose tail is a tail of the absolute path, or a unique file name.
 */
export function resolveResource(uri: string, paths: Iterable<string>): string | null {
  const all = [...paths];
  const dec = (s: string) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };
  const m = /^modelica:\/\/([^/]+)\/(.*)$/i.exec(uri);
  if (m) {
    const pkg = m[1].split(".");
    const rest = dec(m[2]);
    for (let k = pkg.length; k >= 1; k--) {
      const suffix = `/${pkg.slice(0, k).join("/")}/${rest}`;
      const hit = all.find((p) => p.endsWith(suffix));
      if (hit) return hit;
    }
    return null;
  }
  const abs = dec(uri.replace(/^file:\/\//i, "")).replace(/\\/g, "/");
  let best: string | null = null;
  for (const p of all) {
    const rel = p.slice(p.indexOf("/") + 1);
    if ((abs.endsWith("/" + rel) || abs === rel) && (!best || rel.length > best.length)) best = p;
  }
  if (best) return best;
  const base = abs.split("/").pop()!;
  const byName = all.filter((p) => p.split("/").pop() === base);
  return byName.length === 1 ? byName[0] : null;
}

function fromBase64(s: string): ArrayBuffer {
  const b = atob(s.replace(/^data:[^,]*,/, ""));
  const u = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i);
  return u.buffer;
}

/** Parse an STL (binary or ASCII) or OBJ file into one geometry with crease-aware normals. */
export function parseMesh(uri: string, buf: ArrayBuffer): THREE.BufferGeometry | null {
  let g: THREE.BufferGeometry;
  if (/\.obj$/i.test(uri.replace(/[?#].*$/, ""))) {
    const group = new OBJLoader().parse(new TextDecoder().decode(buf));
    group.updateMatrixWorld(true);
    const parts: THREE.BufferGeometry[] = [];
    group.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (!mesh.isMesh) return;
      const p = new THREE.BufferGeometry();
      p.setAttribute("position", mesh.geometry.getAttribute("position").clone());
      if (mesh.geometry.index) p.setIndex(mesh.geometry.index.clone());
      parts.push(p.applyMatrix4(o.matrixWorld));
    });
    if (!parts.length) return null;
    g = parts.length === 1 ? parts[0] : (mergeGeometries(parts.map((p) => (p.index ? p.toNonIndexed() : p))) ?? parts[0]);
  } else {
    const s = new STLLoader().parse(buf);
    g = new THREE.BufferGeometry();
    g.setAttribute("position", s.getAttribute("position"));
  }
  if (!g.getAttribute("position")?.count) return null;
  g.computeBoundingBox();
  const diag = g.boundingBox!.getSize(new THREE.Vector3()).length();
  // weld coincident corners, then split normals at edges sharper than 30 degrees
  g = mergeVertices(g, Math.max(1e-9, diag * 1e-6));
  g = toCreasedNormals(g, Math.PI / 6);
  g = mergeVertices(g, Math.max(1e-9, diag * 1e-6));
  g.computeBoundingBox();
  g.computeBoundingSphere();
  return g;
}

const meshCache = new Map<string, { src: ArrayBuffer | string; g: THREE.BufferGeometry | null }>();
function meshGeometry(uri: string, src: ArrayBuffer | string): THREE.BufferGeometry | null {
  const hit = meshCache.get(uri);
  if (hit && hit.src === src) return hit.g;
  let g: THREE.BufferGeometry | null = null;
  try {
    g = parseMesh(uri, typeof src === "string" ? fromBase64(src) : src);
  } catch {
    g = null;
  }
  meshCache.set(uri, { src, g });
  return g;
}

const at = (x: Track, i: number, f: number) => {
  if (typeof x === "number") return x;
  const a = x[i];
  return f > 0 && i + 1 < x.length ? a + (x[i + 1] - a) * f : a;
};

/** The component that declared a shape: the instance name without the visual itself and the
 *  MultiBody-internal helper holding it (`car.FL.lowerArm.frameTranslation.shape` -> `car.FL.lowerArm`). */
export function componentOf(name: string): string {
  const p = name.split(/\.(?![^[]*\])/);
  if (p.length > 1) p.pop();
  if (p.length > 1 && /^(frameTranslation|translation|body|bodyShape|vis)$/.test(p[p.length - 1])) p.pop();
  return p.join(".");
}

// ------------------------------------------------------------------ materials and environment

function shapeMaterial(rgb: number[], spec: number, doubleSide: boolean): THREE.MeshStandardMaterial {
  const color = new THREE.Color().setRGB(rgb[0] / 255, rgb[1] / 255, rgb[2] / 255, THREE.SRGBColorSpace);
  const side = doubleSide ? THREE.DoubleSide : THREE.FrontSide;
  if (rgb.every((c) => c < 50)) return new THREE.MeshStandardMaterial({ color, roughness: 0.9, metalness: 0, side });
  if (spec >= 0.8) return new THREE.MeshStandardMaterial({ color, roughness: 0.3, metalness: 0.7, side });
  // painted parts: a thin clear coat gives the environment reflections of a lacquered surface
  return new THREE.MeshPhysicalMaterial({ color, roughness: 0.6, metalness: 0.05, clearcoat: 0.35, clearcoatRoughness: 0.25, side });
}

const HORIZON = new THREE.Color(0xdde4ec);

function skyDome(up: THREE.Vector3): THREE.Mesh {
  const mat = new THREE.ShaderMaterial({
    uniforms: {
      up: { value: up },
      zenith: { value: new THREE.Color(0x7fa6d6) },
      horizon: { value: HORIZON },
      ground: { value: new THREE.Color(0xc4c9cf) },
    },
    vertexShader: `varying vec3 vDir;
      void main() {
        vDir = (modelMatrix * vec4(position, 0.0)).xyz;
        gl_Position = projectionMatrix * viewMatrix * modelMatrix * vec4(position, 1.0);
        gl_Position.z = gl_Position.w * 0.99999;
      }`,
    fragmentShader: `uniform vec3 up; uniform vec3 zenith; uniform vec3 horizon; uniform vec3 ground; varying vec3 vDir;
      void main() {
        float h = dot(normalize(vDir), up);
        vec3 c = h > 0.0 ? mix(horizon, zenith, pow(h, 0.55)) : mix(horizon, ground, smoothstep(0.0, 0.4, -h));
        gl_FragColor = vec4(c, 1.0);
        #include <tonemapping_fragment>
        #include <colorspace_fragment>
      }`,
    side: THREE.BackSide,
    depthWrite: false,
    depthTest: false,
    fog: false,
  });
  const m = new THREE.Mesh(new THREE.SphereGeometry(1, 32, 16), mat);
  m.frustumCulled = false;
  m.renderOrder = -1;
  return m;
}

/** One grid floor tile: ten cells with a darker border line. */
function gridTexture(): THREE.CanvasTexture {
  const n = 512, c = document.createElement("canvas");
  c.width = c.height = n;
  const g = c.getContext("2d")!;
  g.fillStyle = "#e3e7ec";
  g.fillRect(0, 0, n, n);
  for (let i = 0; i <= 10; i++) {
    const x = Math.round((i * n) / 10);
    g.fillStyle = i % 10 === 0 ? "#8a95a5" : "#b9c1cc";
    const w = i % 10 === 0 ? 3 : 2;
    g.fillRect(x - w / 2, 0, w, n);
    g.fillRect(0, x - w / 2, n, w);
  }
  return canvasTexture(c);
}

/** Asphalt tile, LANE_X long (along world x) and 2 lanes wide, with dashed lane lines at the
 *  quarter and three-quarter width so the tile seams fall on lane centres. */
const LANE = 3.5, DASH_PERIOD = 12, DASH = 3;
function asphaltTexture(): THREE.CanvasTexture {
  const ppm = 96, W = DASH_PERIOD * ppm, H = 2 * LANE * ppm;
  const c = document.createElement("canvas");
  c.width = W;
  c.height = H;
  const g = c.getContext("2d")!;
  // seeded noise so the texture is the same every time
  let seed = 12345;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  const img = g.createImageData(W, H);
  for (let i = 0; i < W * H; i++) {
    let v = 56 + (rnd() - 0.5) * 16;
    const r = rnd();
    if (r < 0.03) v += 14 + rnd() * 22; // light aggregate
    else if (r < 0.08) v -= 14;
    img.data[4 * i] = v + 2;
    img.data[4 * i + 1] = v + 1;
    img.data[4 * i + 2] = v;
    img.data[4 * i + 3] = 255;
  }
  g.putImageData(img, 0, 0);
  // low-frequency patches (tar, wear), drawn wrapped so the tile repeats seamlessly
  for (let k = 0; k < 60; k++) {
    const x = rnd() * W, y = rnd() * H, r = (0.3 + rnd() * 1.4) * ppm;
    const dark = rnd() < 0.5;
    for (const dx of [-W, 0, W])
      for (const dy of [-H, 0, H]) {
        const gr = g.createRadialGradient(x + dx, y + dy, 0, x + dx, y + dy, r);
        gr.addColorStop(0, dark ? "rgba(20,20,22,0.10)" : "rgba(150,150,150,0.07)");
        gr.addColorStop(1, "rgba(0,0,0,0)");
        g.fillStyle = gr;
        g.fillRect(x + dx - r, y + dy - r, 2 * r, 2 * r);
      }
  }
  // lane lines, slightly worn
  const lw = 0.12 * ppm;
  for (const y of [H / 4, (3 * H) / 4]) {
    g.fillStyle = "rgba(236,236,230,0.92)";
    g.fillRect(0, y - lw / 2, DASH * ppm, lw);
    for (let i = 0; i < DASH * ppm * lw * 0.08; i++) {
      g.fillStyle = `rgba(70,70,70,${0.3 + rnd() * 0.4})`;
      g.fillRect(rnd() * DASH * ppm, y - lw / 2 + rnd() * lw, 1.5, 1.5);
    }
  }
  return canvasTexture(c);
}

function canvasTexture(c: HTMLCanvasElement) {
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.RepeatWrapping;
  t.anisotropy = 8;
  return t;
}

// ------------------------------------------------------------------ view

export type FloorMode = "grid" | "asphalt" | "none";
export type FollowMode = "off" | "follow" | "chase";
export type ViewPreset = "iso" | "front" | "side" | "rear" | "top";

interface Entry {
  s: AnimShape;
  m: THREE.Mesh;
  mat: THREE.MeshStandardMaterial;
  moving: boolean;
  /** External mesh: drawn at its own size unless extra = 1. */
  file: boolean;
}

/** Trajectory of the centre of the moving parts on a uniform time grid, with the smoothed
 *  heading of its motion in the floor plane (used by the chase camera). */
interface CentreTrack {
  t0: number;
  dt: number;
  c: Float64Array;
  yaw: Float64Array;
}

interface Tween {
  start: number;
  dur: number;
  /** camera offset from the target, as (azimuth, elevation, distance) in the floor basis */
  from: [number, number, number];
  to: [number, number, number];
  tFrom: THREE.Vector3;
  tTo: THREE.Vector3;
}

export class Animation3D {
  readonly renderer: THREE.WebGLRenderer;
  readonly scene = new THREE.Scene();
  readonly camera = new THREE.PerspectiveCamera(40, 1, 0.01, 5000);
  readonly controls: OrbitControls;
  /** Called with one line when something in the data cannot be shown as intended. */
  onWarning?: (msg: string) => void;
  /** Called when a recording is ready (the bar also offers it as a download link). */
  onRecorded?: (video: Blob, fileName: string) => void;
  private root = new THREE.Group();
  private meshes: Entry[] = [];
  private data: AnimData | null = null;
  private ground: THREE.Mesh;
  private groundMat: THREE.MeshStandardMaterial;
  private gridTex = gridTexture();
  private asphaltTex: THREE.CanvasTexture | null = null;
  /** Floor tile size along the floor's x and y axes; the floor moves in whole tiles so its pattern stays put. */
  private tileX = 1;
  private tileY = 1;
  private groundH = 0;
  private diag = 1;
  floorMode: FloorMode = "grid";
  private floorUser = false;
  followMode: FollowMode = "follow";
  private sky: THREE.Mesh;
  private sun: THREE.DirectionalLight;
  private up = new THREE.Vector3(0, 0, 1);
  /** World x and the lateral axis up × x, both in the floor plane. */
  private fwd = new THREE.Vector3(1, 0, 0);
  private lat = new THREE.Vector3(0, 1, 0);
  private bar: HTMLElement;
  private slider: HTMLInputElement;
  private label: HTMLElement;
  private info: HTMLElement;
  private playBtn: HTMLButtonElement;
  private speedSel: HTMLSelectElement;
  private followSel: HTMLSelectElement;
  private floorSel: HTMLSelectElement;
  private xrayBtn: HTMLButtonElement;
  private recBtn: HTMLButtonElement;
  private dlLink: HTMLAnchorElement;
  private canvasHost: HTMLElement;
  /** Current simulation time. */
  t = 0;
  playing = false;
  private last = 0;
  private track: CentreTrack | null = null;
  /** Distance the moving parts travel over the result. */
  travel = 0;
  private followPrev: THREE.Vector3 | null = null;
  private appliedYaw: number | null = null;
  private tween: Tween | null = null;
  private dirty = true;
  private raf = 0;
  private selected: Entry | null = null;
  private xray = new Set<string>();
  private recorder: MediaRecorder | null = null;

  constructor(readonly host: HTMLElement) {
    host.classList.add("anim3d");
    this.canvasHost = document.createElement("div");
    this.canvasHost.className = "anim-canvas";
    host.appendChild(this.canvasHost);
    this.renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
    this.renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = 1.0;
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.canvasHost.appendChild(this.renderer.domElement);
    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.12;
    this.controls.addEventListener("change", () => (this.dirty = true));
    this.controls.addEventListener("start", () => (this.tween = null));

    const pmrem = new THREE.PMREMGenerator(this.renderer);
    this.scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    this.scene.environmentIntensity = 0.5;
    pmrem.dispose();
    this.scene.background = HORIZON.clone();
    this.sky = skyDome(this.up);
    this.scene.add(this.sky);
    this.scene.add(new THREE.HemisphereLight(0xeef4ff, 0x8a8478, 0.55));
    this.sun = new THREE.DirectionalLight(0xfff6e8, 3.0);
    this.sun.castShadow = true;
    this.sun.shadow.mapSize.set(2048, 2048);
    this.sun.shadow.bias = -0.0003;
    this.scene.add(this.sun, this.sun.target);
    const aniso = this.renderer.capabilities.getMaxAnisotropy();
    this.gridTex.anisotropy = aniso;
    this.groundMat = new THREE.MeshStandardMaterial({ map: this.gridTex, roughness: 0.95, metalness: 0, envMapIntensity: 0.4 });
    this.ground = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), this.groundMat);
    this.ground.receiveShadow = true;
    this.scene.add(this.ground, this.root);

    // playback bar
    this.bar = document.createElement("div");
    this.bar.className = "anim-bar";
    const canRecord = typeof MediaRecorder !== "undefined" && typeof this.renderer.domElement.captureStream === "function";
    this.bar.innerHTML = `
      <span class="anim-grp">
        <button data-a="start" title="Rewind (Home)">⏮</button>
        <button data-a="play" title="Play / pause (space); ← → step one output interval">▶</button>
      </span>
      <input type="range" min="0" max="1000" value="0" class="anim-slider" />
      <span class="anim-time">t = 0 s</span>
      <label>Speed <select class="anim-speed"><option>0.1</option><option>0.25</option><option>0.5</option><option selected>1</option><option>2</option><option>5</option><option>10</option><option>20</option><option>50</option><option>100</option></select>×</label>
      <span class="seg anim-views" title="Camera direction">
        <button data-v="iso" title="Three-quarter view">Iso</button><button data-v="front" title="From the front (+x)">Front</button><button data-v="side" title="From the side (−y)">Side</button><button data-v="rear" title="From behind (−x)">Rear</button><button data-v="top" title="From above">Top</button>
      </span>
      <label title="Follow: move with the moving parts; Chase: also turn with their direction of travel">Camera <select class="anim-follow"><option value="off">Off</option><option value="follow" selected>Follow</option><option value="chase">Chase</option></select></label>
      <label>Floor <select class="anim-floor"><option value="grid">Grid</option><option value="asphalt">Asphalt</option><option value="none">None</option></select></label>
      <button data-a="fit" title="Fit the view">Fit</button>
      <span class="seg">
        <button data-a="xray" title="Make the selected component transparent (click a part to select it)">X-ray</button><button data-a="showall" title="Make all parts opaque again">Show all</button>
      </span>
      <button data-a="rec" title="Play from the start and record a video"${canRecord ? "" : ' style="display:none"'}>● Record</button>
      <a class="anim-dl" style="display:none"></a>
      <span class="spacer"></span>
      <span class="anim-info muted"></span>`;
    host.appendChild(this.bar);
    const q = <T extends Element>(s: string) => this.bar.querySelector(s) as T;
    this.slider = q(".anim-slider");
    this.label = q(".anim-time");
    this.info = q(".anim-info");
    this.playBtn = q('[data-a="play"]');
    this.speedSel = q(".anim-speed");
    this.followSel = q(".anim-follow");
    this.floorSel = q(".anim-floor");
    this.xrayBtn = q('[data-a="xray"]');
    this.recBtn = q('[data-a="rec"]');
    this.dlLink = q(".anim-dl");
    q<HTMLButtonElement>('[data-a="start"]').onclick = () => this.seek(this.t0());
    this.playBtn.onclick = () => this.toggle();
    q<HTMLButtonElement>('[data-a="fit"]').onclick = () => this.fit();
    this.xrayBtn.onclick = () => this.toggleXray();
    q<HTMLButtonElement>('[data-a="showall"]').onclick = () => this.showAll();
    this.recBtn.onclick = () => this.record();
    this.bar.querySelectorAll<HTMLButtonElement>("[data-v]").forEach((b) => (b.onclick = () => this.view(b.dataset.v as ViewPreset)));
    this.followSel.onchange = () => this.setFollow(this.followSel.value as FollowMode);
    this.floorSel.onchange = () => {
      this.floorUser = true;
      this.setFloor(this.floorSel.value as FloorMode);
    };
    this.slider.oninput = () => {
      const tt = this.data?.time ?? [0];
      this.seek(this.t0() + ((tt[tt.length - 1] - this.t0()) * +this.slider.value) / 1000);
    };
    host.tabIndex = 0;
    host.addEventListener("keydown", (e) => {
      if ((e.target as HTMLElement).tagName === "SELECT") return;
      if (e.key === " ") this.toggle();
      else if (e.key === "ArrowLeft" || e.key === "ArrowRight") this.step(e.key === "ArrowLeft" ? -1 : 1);
      else if (e.key === "Home") this.seek(this.t0());
      else return;
      e.preventDefault();
    });

    // click (without dragging) to select a part
    const dom = this.renderer.domElement;
    let down: [number, number] | null = null;
    dom.addEventListener("pointerdown", (e) => {
      down = [e.clientX, e.clientY];
      host.focus({ preventScroll: true });
    });
    dom.addEventListener("pointerup", (e) => {
      if (down && e.button === 0 && Math.hypot(e.clientX - down[0], e.clientY - down[1]) < 5) this.pick(e.clientX, e.clientY);
      down = null;
    });
    new ResizeObserver(() => this.resize()).observe(this.canvasHost);
    this.last = performance.now();
    this.raf = requestAnimationFrame(this.loop);
  }

  private t0() {
    return this.data?.time[0] ?? 0;
  }

  get hasData() {
    return !!this.data && this.data.shapes.length > 0;
  }

  setData(d: AnimData) {
    this.stop();
    this.data = d;
    // old materials are disposed after the first frame, so their shader programs are reused
    const old = this.meshes.map((e) => e.mat);
    for (const { m } of this.meshes) this.root.remove(m);
    this.meshes = [];
    this.selected = null;
    this.xray.clear();
    this.tween = null;
    const g = d.gravity.length === 3 && Math.hypot(d.gravity[0], d.gravity[1], d.gravity[2]) > 0 ? d.gravity : [0, -1, 0];
    this.up.set(-g[0], -g[1], -g[2]).normalize();
    this.camera.up.copy(this.up);
    this.controls.object.up.copy(this.up);
    // OrbitControls caches the rotation into its "y up" frame when it is created
    const oc = this.controls as unknown as { _quat?: THREE.Quaternion; _quatInverse?: THREE.Quaternion };
    if (oc._quat && oc._quatInverse) {
      oc._quat.setFromUnitVectors(this.up, new THREE.Vector3(0, 1, 0));
      oc._quatInverse.copy(oc._quat).invert();
    }
    this.fwd.set(1, 0, 0);
    if (Math.abs(this.fwd.dot(this.up)) > 0.9) this.fwd.set(0, 1, 0);
    this.fwd.addScaledVector(this.up, -this.fwd.dot(this.up)).normalize();
    this.lat.crossVectors(this.up, this.fwd);
    const missing = new Set<string>();
    for (const s of d.shapes) {
      const extra = at(s.extra, 0, 0);
      const file = isFileShape(s.type);
      let geom: THREE.BufferGeometry | null = null;
      if (file) {
        const src = d.meshes?.[s.type];
        geom = src ? meshGeometry(s.type, src) : null;
        if (!geom) missing.add(s.type);
      }
      const wire = s.type === "spring" ? Math.min(0.45, Math.max(0.02, at(s.height, 0, 0) / Math.max(1e-9, at(s.width, 0, 0)))) : 0;
      const col = s.color.map((c) => Math.max(0, Math.min(255, at(c, 0, 0))));
      const mat = shapeMaterial(col, at(s.specularCoefficient, 0, 0), s.type === "pipe" || file);
      const m = new THREE.Mesh(geom ?? unitGeometry(file ? "box" : s.type, extra, wire), mat);
      m.matrixAutoUpdate = false;
      m.castShadow = true;
      m.receiveShadow = true;
      m.name = s.name;
      const moving = [...s.r, ...s.T, ...s.r_shape, ...s.lengthDirection, ...s.widthDirection, s.length, s.width, s.height].some((x) => typeof x !== "number");
      const e: Entry = { s, m, mat, moving, file: !!geom };
      m.userData.entry = e;
      this.meshes.push(e);
      this.root.add(m);
    }
    if (missing.size) this.onWarning?.(`Animation: ${missing.size} shape file${missing.size > 1 ? "s" : ""} not found or unreadable, drawn as boxes: ${[...missing].join(", ")}`);
    const tt = d.time;
    const span = tt.length ? tt[tt.length - 1] - tt[0] : 0;
    this.t = tt[0] ?? 0;
    // A slow-motion default for short transients, real time otherwise.
    // short runs slow down; long drive cycles speed up so a replay lasts about a minute at most
    this.speedSel.value = span > 0 && span < 1 ? "0.1" : span > 600 ? "100" : span > 300 ? "20" : span > 60 ? "10" : "1";
    this.buildTrack();
    if (!this.floorUser) {
      this.floorMode = this.travel > 5 && this.roams() ? "asphalt" : "grid";
      this.floorSel.value = this.floorMode;
    }
    this.place(this.t);
    this.showInfo();
    this.updateXrayBtn();
    this.fit();
    for (const m of old) m.dispose();
  }

  private index(t: number): [number, number] {
    const tt = this.data!.time;
    if (tt.length < 2 || t <= tt[0]) return [0, 0];
    if (t >= tt[tt.length - 1]) return [tt.length - 1, 0];
    let lo = 0, hi = tt.length - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (tt[mid] <= t) lo = mid;
      else hi = mid;
    }
    const dt = tt[hi] - tt[lo];
    return [lo, dt > 0 ? (t - tt[lo]) / dt : 0];
  }

  private place(t: number) {
    if (!this.data) return;
    const [i, f] = this.index(t);
    const ex = new THREE.Vector3(), ey = new THREE.Vector3(), ez = new THREE.Vector3(), wd = new THREE.Vector3();
    const R = new THREE.Matrix3();
    const v = new THREE.Vector3();
    for (const { s, m, file } of this.meshes) {
      const val = (x: Track) => at(x, i, f);
      ex.set(val(s.lengthDirection[0]), val(s.lengthDirection[1]), val(s.lengthDirection[2]));
      if (ex.lengthSq() < 1e-20) ex.set(1, 0, 0);
      ex.normalize();
      wd.set(val(s.widthDirection[0]), val(s.widthDirection[1]), val(s.widthDirection[2]));
      ez.crossVectors(ex, wd);
      if (ez.lengthSq() < 1e-12) {
        // widthDirection parallel to lengthDirection: pick any perpendicular, as MSL does
        wd.set(Math.abs(ex.x) < 0.9 ? 1 : 0, Math.abs(ex.x) < 0.9 ? 0 : 1, 0);
        ez.crossVectors(ex, wd);
      }
      ez.normalize();
      ey.crossVectors(ez, ex);
      // R.T maps world to local, so its transpose maps local to world.
      const T = s.T.map(val);
      R.set(T[0], T[3], T[6], T[1], T[4], T[7], T[2], T[5], T[8]);
      let L = val(s.length), W = val(s.width);
      // a spring's height is its wire diameter; the coil is round with diameter width
      let H = s.type === "spring" ? W : val(s.height);
      if (file && Math.round(val(s.extra)) !== 1) L = W = H = 1;
      const cx = ex.applyMatrix3(R).multiplyScalar(L);
      const cy = ey.applyMatrix3(R).multiplyScalar(W);
      const cz = ez.applyMatrix3(R).multiplyScalar(H);
      v.set(val(s.r_shape[0]), val(s.r_shape[1]), val(s.r_shape[2])).applyMatrix3(R);
      v.x += val(s.r[0]);
      v.y += val(s.r[1]);
      v.z += val(s.r[2]);
      m.matrix.set(cx.x, cy.x, cz.x, v.x, cx.y, cy.y, cz.y, v.y, cx.z, cy.z, cz.z, v.z, 0, 0, 0, 1);
      m.matrixWorldNeedsUpdate = true;
      m.visible = Math.abs(L) + Math.abs(W) + Math.abs(H) > 0;
    }
  }

  private movingCentre(): THREE.Vector3 | null {
    const box = new THREE.Box3();
    const p = new THREE.Vector3();
    let n = 0;
    for (const { m, moving } of this.meshes) {
      if (!moving || !m.visible) continue;
      box.expandByPoint(p.setFromMatrixPosition(m.matrix));
      n++;
    }
    return n ? box.getCenter(new THREE.Vector3()) : null;
  }

  /** Sample the centre of the moving parts over the result and derive the smoothed heading. */
  private buildTrack() {
    this.track = null;
    this.travel = 0;
    const tt = this.data!.time;
    if (tt.length < 2 || !this.meshes.some((e) => e.moving)) return;
    const t0 = tt[0], span = tt[tt.length - 1] - t0;
    if (!(span > 0)) return;
    const M = Math.max(2, Math.min(1500, Math.ceil(span / 0.01) + 1));
    const dt = span / (M - 1);
    const c = new Float64Array(3 * M);
    let prev = new THREE.Vector3();
    for (let i = 0; i < M; i++) {
      this.place(t0 + i * dt);
      const p = this.movingCentre() ?? prev;
      c[3 * i] = p.x;
      c[3 * i + 1] = p.y;
      c[3 * i + 2] = p.z;
      prev = p;
      this.travel = Math.max(this.travel, Math.hypot(p.x - c[0], p.y - c[1], p.z - c[2]));
    }
    const yaw = new Float64Array(M).fill(NaN);
    const v = new THREE.Vector3();
    for (let i = 0; i < M; i++) {
      const a = Math.max(0, i - 1), b = Math.min(M - 1, i + 1);
      v.set(c[3 * b] - c[3 * a], c[3 * b + 1] - c[3 * a + 1], c[3 * b + 2] - c[3 * a + 2]).divideScalar((b - a) * dt);
      const vf = v.dot(this.fwd), vl = v.dot(this.lat);
      if (Math.hypot(vf, vl) >= 0.5) yaw[i] = Math.atan2(vl, vf);
    }
    // below 0.5 m/s keep the last heading (the chase camera then just follows)
    let first = yaw.findIndex((y) => !isNaN(y));
    if (first < 0) yaw.fill(0), (first = 0);
    for (let i = 0; i < M; i++) {
      if (i < first) yaw[i] = yaw[first];
      else if (isNaN(yaw[i])) yaw[i] = yaw[i - 1];
      else if (i > 0) yaw[i] = yaw[i - 1] + Math.atan2(Math.sin(yaw[i] - yaw[i - 1]), Math.cos(yaw[i] - yaw[i - 1]));
    }
    // moving average over half a second
    const w = Math.max(1, Math.round(0.25 / dt));
    const cum = new Float64Array(M + 1);
    for (let i = 0; i < M; i++) cum[i + 1] = cum[i] + yaw[i];
    const sm = new Float64Array(M);
    for (let i = 0; i < M; i++) {
      const a = Math.max(0, i - w), b = Math.min(M - 1, i + w);
      sm[i] = (cum[b + 1] - cum[a]) / (b - a + 1);
    }
    this.track = { t0, dt, c, yaw: sm };
  }

  private trackAt(t: number): [THREE.Vector3, number] | null {
    const k = this.track;
    if (!k) return null;
    const M = k.yaw.length;
    const x = Math.max(0, Math.min(M - 1, (t - k.t0) / k.dt));
    const i = Math.min(M - 2, Math.floor(x)), f = x - i;
    const l = (a: Float64Array, s: number, o: number) => a[s * i + o] + (a[s * (i + 1) + o] - a[s * i + o]) * f;
    return [new THREE.Vector3(l(k.c, 3, 0), l(k.c, 3, 1), l(k.c, 3, 2)), l(k.yaw, 1, 0)];
  }

  /** Whether the moving parts travel further than the scene is big (a car on a road), rather
   *  than turning or stroking in place (a rig, a test bench). */
  private roams() {
    this.root.updateMatrixWorld(true);
    const all = new THREE.Box3();
    for (const { m } of this.meshes) if (m.visible) all.expandByObject(m);
    return !all.isEmpty() && this.travel > all.getSize(new THREE.Vector3()).length() * 0.5;
  }

  /** Bounding box of what the camera should frame. */
  private frameBox() {
    this.root.updateMatrixWorld(true);
    const box = new THREE.Box3();
    const roams = this.roams();
    const anyMoving = this.meshes.some((x) => x.moving);
    for (const { m, moving } of this.meshes) if (m.visible && (moving || this.followMode === "off" || !anyMoving || !roams)) box.expandByObject(m);
    if (box.isEmpty()) for (const { m } of this.meshes) if (m.visible) box.expandByObject(m);
    if (box.isEmpty()) box.setFromCenterAndSize(new THREE.Vector3(), new THREE.Vector3(1, 1, 1));
    return box;
  }

  /** Camera direction (from the target) of a preset, in world coordinates. */
  private presetDir(p: ViewPreset) {
    const f = this.fwd, l = this.lat, u = this.up;
    const d = new THREE.Vector3();
    const el = 0.2;
    if (p === "front") d.copy(f).addScaledVector(u, el);
    else if (p === "rear") d.copy(f).negate().addScaledVector(u, el);
    else if (p === "side") d.copy(l).negate().addScaledVector(u, el);
    else if (p === "top") d.copy(u).addScaledVector(f, -0.002);
    else d.copy(f).addScaledVector(l, -0.9).addScaledVector(u, 0.55);
    // the chase camera's presets are relative to the direction of travel
    if (this.followMode === "chase" && this.appliedYaw !== null) d.applyAxisAngle(u, this.appliedYaw);
    return d.normalize();
  }

  /** Frame the whole model (or, when following, the moving parts) from a three-quarter front view. */
  fit() {
    this.view("iso", false);
  }

  /** Look at the model from a preset direction, animating the camera unless `animate` is false. */
  view(p: ViewPreset, animate = true) {
    if (!this.data) return;
    this.place(this.t);
    const box = this.frameBox();
    const c = box.getCenter(new THREE.Vector3());
    const size = box.getSize(new THREE.Vector3()).length() || 1;
    const k = this.trackAt(this.t);
    this.followPrev = this.followMode !== "off" && k ? k[0] : null;
    this.appliedYaw = this.followMode === "chase" && k ? k[1] : null;
    const dist = size * (p === "iso" ? 1.25 : 1.1);
    const pos = c.clone().addScaledVector(this.presetDir(p), dist);
    this.camera.near = size / 200;
    this.camera.far = size * 400;
    this.camera.updateProjectionMatrix();
    this.layoutGround(box);
    if (animate && this.camera.position.distanceTo(this.controls.target) > 0) {
      this.tween = {
        start: performance.now(),
        dur: 600,
        from: this.sph(this.camera.position.clone().sub(this.controls.target)),
        to: this.sph(pos.clone().sub(c)),
        tFrom: this.controls.target.clone(),
        tTo: c,
      };
    } else {
      this.tween = null;
      this.camera.position.copy(pos);
      this.controls.target.copy(c);
      this.controls.update();
    }
    this.render();
  }

  private sph(o: THREE.Vector3): [number, number, number] {
    const r = o.length() || 1;
    return [Math.atan2(o.dot(this.lat), o.dot(this.fwd)), Math.asin(Math.max(-1, Math.min(1, o.dot(this.up) / r))), r];
  }

  private unsph([az, el, r]: [number, number, number]) {
    return this.fwd.clone().multiplyScalar(Math.cos(az) * Math.cos(el)).addScaledVector(this.lat, Math.sin(az) * Math.cos(el)).addScaledVector(this.up, Math.sin(el)).multiplyScalar(r);
  }

  private stepTween(now: number) {
    const w = this.tween!;
    const x = Math.min(1, (now - w.start) / w.dur);
    const k = x * x * (3 - 2 * x);
    let daz = w.to[0] - w.from[0];
    daz = Math.atan2(Math.sin(daz), Math.cos(daz));
    const o = this.unsph([w.from[0] + daz * k, w.from[1] + (w.to[1] - w.from[1]) * k, w.from[2] + (w.to[2] - w.from[2]) * k]);
    this.controls.target.lerpVectors(w.tFrom, w.tTo, k);
    this.camera.position.copy(this.controls.target).add(o);
    this.controls.update();
    this.dirty = true;
    if (x >= 1) this.tween = null;
  }

  private layoutGround(box: THREE.Box3) {
    // ground plane just below the lowest point of the model, normal to "up"
    let low = Infinity;
    for (const { m } of this.meshes) {
      if (!m.visible) continue;
      const b = new THREE.Box3().setFromObject(m);
      for (const x of [b.min, b.max]) low = Math.min(low, x.dot(this.up));
    }
    if (!isFinite(low)) low = 0;
    this.groundH = low - 1e-3;
    this.diag = Math.max(0.5, box.getSize(new THREE.Vector3()).length());
    // floor axes: local x along world x (the lane direction), normal along up
    const basis = new THREE.Matrix4().makeBasis(this.fwd, this.lat, this.up);
    this.ground.quaternion.setFromRotationMatrix(basis);
    this.applyFloor(box.getCenter(new THREE.Vector3()));
    // sun above and to the side, shadow box around the model
    const c = box.getCenter(new THREE.Vector3());
    const r = Math.max(0.3, box.getSize(new THREE.Vector3()).length());
    const dir = this.up.clone().multiplyScalar(3).addScaledVector(this.fwd, 0.6).addScaledVector(this.lat, -0.9).normalize();
    this.sun.position.copy(c).addScaledVector(dir, r * 3);
    this.sun.target.position.copy(c);
    const cam = this.sun.shadow.camera;
    cam.left = cam.bottom = -r;
    cam.right = cam.top = r;
    cam.near = 0.01;
    cam.far = r * 8;
    cam.updateProjectionMatrix();
    this.sun.shadow.normalBias = (2 * r) / 2048 * 1.5;
    this.sun.shadow.needsUpdate = true;
  }

  private applyFloor(centre: THREE.Vector3) {
    const mode = this.floorMode;
    this.ground.visible = mode !== "none";
    let size = this.diag * 24;
    if (mode === "asphalt") {
      if (!this.asphaltTex) {
        this.asphaltTex = asphaltTexture();
        this.asphaltTex.anisotropy = this.renderer.capabilities.getMaxAnisotropy();
      }
      this.groundMat.map = this.asphaltTex;
      this.groundMat.roughness = 0.88;
      this.tileX = DASH_PERIOD;
      this.tileY = 2 * LANE;
      size = Math.max(size, 160);
    } else {
      // grid cells of 1, 10 or 100 mm, cm, m...: about a tenth of the model's size
      this.groundMat.map = this.gridTex;
      this.groundMat.roughness = 0.95;
      this.tileX = this.tileY = 10 * Math.pow(10, Math.floor(Math.log10(this.diag / 2.5)));
    }
    this.groundMat.needsUpdate = true;
    const nx = 2 * Math.ceil(size / this.tileX / 2), ny = 2 * Math.ceil(size / this.tileY / 2);
    this.ground.scale.set(nx * this.tileX, ny * this.tileY, 1);
    this.groundMat.map!.repeat.set(nx, ny);
    this.ground.position.copy(this.snap(centre));
    // fade the far floor into the horizon
    this.scene.fog = new THREE.Fog(HORIZON, size * 0.12, size * 0.46);
  }

  /** Point on the floor below `p`, rounded to whole tiles. */
  private snap(p: THREE.Vector3) {
    const u = Math.round(p.dot(this.fwd) / this.tileX) * this.tileX;
    const v = Math.round(p.dot(this.lat) / this.tileY) * this.tileY;
    return this.fwd.clone().multiplyScalar(u).addScaledVector(this.lat, v).addScaledVector(this.up, this.groundH);
  }

  setFloor(mode: FloorMode) {
    this.floorMode = mode;
    this.floorSel.value = mode;
    if (!this.data) return;
    const k = this.trackAt(this.t);
    this.applyFloor(k && this.followMode !== "off" ? k[0] : this.ground.position);
    this.dirty = true;
  }

  setFollow(mode: FollowMode) {
    this.followMode = mode;
    this.followSel.value = mode;
    const k = this.trackAt(this.t);
    this.followPrev = mode !== "off" && k ? k[0] : null;
    this.appliedYaw = mode === "chase" && k ? k[1] : null;
    this.dirty = true;
  }

  private follow() {
    const k = this.followMode === "off" ? null : this.trackAt(this.t);
    if (!k) {
      this.followPrev = null;
      this.appliedYaw = null;
      return;
    }
    const [c, yaw] = k;
    if (this.followPrev) {
      const d = c.clone().sub(this.followPrev);
      this.camera.position.add(d);
      this.controls.target.add(d);
      this.sun.position.add(d);
      this.sun.target.position.add(d);
      if (this.tween) {
        this.tween.tFrom.add(d);
        this.tween.tTo.add(d);
      }
    }
    this.followPrev = c;
    if (this.followMode === "chase") {
      if (this.appliedYaw !== null && yaw !== this.appliedYaw) {
        const dy = yaw - this.appliedYaw;
        const o = this.camera.position.clone().sub(this.controls.target).applyAxisAngle(this.up, dy);
        this.camera.position.copy(this.controls.target).add(o);
        if (this.tween) {
          this.tween.from[0] += dy;
          this.tween.to[0] += dy;
        }
      }
      this.appliedYaw = yaw;
    } else this.appliedYaw = null;
    // keep the floor under the model
    this.ground.position.copy(this.snap(c));
  }

  seek(t: number) {
    if (!this.data) return;
    const tt = this.data.time;
    const t1 = tt[tt.length - 1] ?? 0;
    this.t = Math.max(tt[0] ?? 0, Math.min(t1, t));
    this.place(this.t);
    this.follow();
    this.controls.update();
    this.slider.value = String(t1 > tt[0] ? Math.round((1000 * (this.t - tt[0])) / (t1 - tt[0])) : 0);
    this.label.textContent = `t = ${this.t.toFixed(3)} s`;
    this.render();
  }

  /** Move to the next (n > 0) or previous output point. */
  step(n: number) {
    if (!this.data) return;
    this.stop();
    const tt = this.data.time;
    const eps = 1e-9 * Math.max(1, Math.abs(this.t));
    let [i] = this.index(this.t);
    if (n > 0) {
      while (i < tt.length - 1 && tt[i] <= this.t + eps) i++;
    } else {
      while (i > 0 && tt[i] >= this.t - eps) i--;
    }
    this.seek(tt[i]);
  }

  toggle() {
    if (this.playing) this.stop();
    else this.play();
  }

  play() {
    if (!this.data) return;
    const tt = this.data.time;
    if (this.t >= tt[tt.length - 1]) this.seek(tt[0]);
    this.playing = true;
    this.playBtn.textContent = "⏸";
    this.last = performance.now();
  }

  stop() {
    this.playing = false;
    this.playBtn.textContent = "▶";
    if (this.recorder?.state === "recording") this.recorder.stop();
  }

  private loop = (now: number) => {
    this.raf = requestAnimationFrame(this.loop);
    const dt = Math.max(0, Math.min(0.25, (now - this.last) / 1000));
    this.last = now;
    if (this.playing && this.data) {
      const tt = this.data.time;
      const t1 = tt[tt.length - 1];
      this.seek(Math.min(t1, this.t + dt * +this.speedSel.value));
      if (this.t >= t1) this.stop();
    }
    if (this.tween) this.stepTween(now);
    if (this.controls.update()) this.dirty = true;
    if (this.dirty) this.render();
  };

  // ---------------------------------------------------------------- inspection

  private pick(x: number, y: number) {
    const r = this.renderer.domElement.getBoundingClientRect();
    const ray = new THREE.Raycaster();
    ray.setFromCamera(new THREE.Vector2(((x - r.left) / r.width) * 2 - 1, -((y - r.top) / r.height) * 2 + 1), this.camera);
    this.root.updateMatrixWorld(true);
    const hit = ray.intersectObjects(this.root.children, false).find((h) => h.object.visible);
    this.select(hit ? (hit.object.userData.entry as Entry).s.name : null);
  }

  /** Select a shape by its instance name (null clears the selection). */
  select(name: string | null) {
    if (this.selected) this.selected.mat.emissive.setRGB(0, 0, 0);
    this.selected = this.meshes.find((e) => e.s.name === name) ?? null;
    if (this.selected) {
      this.selected.mat.emissive.set(0x1f6fff);
      this.selected.mat.emissiveIntensity = 0.45;
    }
    this.showInfo();
    this.updateXrayBtn();
    this.dirty = true;
  }

  get selection(): string | null {
    return this.selected?.s.name ?? null;
  }

  private showInfo() {
    if (!this.data) return;
    const s = this.selected?.s;
    this.info.textContent = s ? `${componentOf(s.name)}  ·  ${s.type}` : `${this.data.shapes.length} shapes · ${this.data.model}`;
    this.info.title = s ? s.name : "";
    this.info.classList.toggle("muted", !s);
  }

  /** Toggle transparency of the selected part's component. */
  toggleXray() {
    if (!this.selected) return;
    const c = componentOf(this.selected.s.name);
    if (this.xray.has(c)) this.xray.delete(c);
    else this.xray.add(c);
    this.applyXray();
  }

  showAll() {
    this.xray.clear();
    this.applyXray();
  }

  private applyXray() {
    const pre = [...this.xray];
    for (const { s, mat } of this.meshes) {
      const x = pre.some((p) => s.name === p || s.name.startsWith(p + "."));
      if (mat.transparent !== x) {
        mat.transparent = x;
        mat.opacity = x ? 0.25 : 1;
        mat.depthWrite = !x;
        mat.needsUpdate = true;
      }
    }
    this.updateXrayBtn();
    this.dirty = true;
  }

  private updateXrayBtn() {
    this.xrayBtn.disabled = !this.selected;
    this.xrayBtn.classList.toggle("active", !!this.selected && this.xray.has(componentOf(this.selected.s.name)));
  }

  // ---------------------------------------------------------------- recording

  /** Play from the start and record the canvas as a WebM video; calling it again stops early. */
  record() {
    if (this.recorder) {
      this.stop();
      return;
    }
    if (!this.data || typeof MediaRecorder === "undefined") return;
    const stream = this.renderer.domElement.captureStream(30);
    const type = ["video/webm;codecs=vp9", "video/webm;codecs=vp8", "video/webm"].find((x) => MediaRecorder.isTypeSupported(x));
    const rec = new MediaRecorder(stream, { ...(type ? { mimeType: type } : {}), videoBitsPerSecond: 8e6 });
    const chunks: Blob[] = [];
    rec.ondataavailable = (e) => {
      if (e.data.size) chunks.push(e.data);
    };
    rec.onstop = () => {
      stream.getTracks().forEach((k) => k.stop());
      this.recorder = null;
      this.recBtn.textContent = "● Record";
      this.recBtn.classList.remove("active");
      const blob = new Blob(chunks, { type: "video/webm" });
      const name = `${(this.data?.model ?? "animation").split(".").pop()}.webm`;
      if (this.dlLink.href) URL.revokeObjectURL(this.dlLink.href);
      this.dlLink.href = URL.createObjectURL(blob);
      this.dlLink.download = name;
      this.dlLink.textContent = `⤓ ${name} (${(blob.size / 1e6).toFixed(1)} MB)`;
      this.dlLink.style.display = "";
      this.onRecorded?.(blob, name);
    };
    this.recorder = rec;
    this.recBtn.textContent = "■ Stop";
    this.recBtn.classList.add("active");
    this.seek(this.t0());
    rec.start(500);
    this.play();
  }

  resize() {
    const w = this.canvasHost.clientWidth, h = this.canvasHost.clientHeight;
    if (!w || !h) return;
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.render();
  }

  /** Stop the render loop and release the GPU resources. */
  dispose() {
    this.stop();
    cancelAnimationFrame(this.raf);
    this.controls.dispose();
    this.renderer.dispose();
  }

  render() {
    this.dirty = false;
    this.sky.position.copy(this.camera.position);
    this.renderer.render(this.scene, this.camera);
  }
}
