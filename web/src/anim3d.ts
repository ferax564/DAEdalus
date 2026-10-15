// 3-D animation of Modelica.Mechanics.MultiBody results, in the style of Dymola's animation window.
//
// Every MultiBody visual (Body cylinders and spheres, joint cylinders, springs, FixedShape, world
// axes) is an instance of Visualizers.Advanced.Shape. Its inputs r, R.T, r_shape, lengthDirection,
// widthDirection, length, width, height, color and extra are ordinary variables of the result, so
// the view needs no model-specific code: it finds each shape in the variable list, reads its
// trajectories and draws it. Only `shapeType` is a String, which the result does not carry; it is
// resolved from the class definitions by `resolveShapeTypes`.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";

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
      g = new THREE.SphereGeometry(0.5, 32, 16).translate(0.5, 0, 0);
      break;
    case "cylinder":
    case "gearwheel":
      g = new THREE.CylinderGeometry(0.5, 0.5, 1, 32).applyMatrix4(toX).translate(0.5, 0, 0);
      break;
    case "cone": {
      const top = Math.max(0, Math.min(1, extra)) * 0.5;
      g = new THREE.CylinderGeometry(top, 0.5, 1, 32).applyMatrix4(toX).translate(0.5, 0, 0);
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

const at = (x: Track, i: number, f: number) => {
  if (typeof x === "number") return x;
  const a = x[i];
  return f > 0 && i + 1 < x.length ? a + (x[i + 1] - a) * f : a;
};

// ------------------------------------------------------------------ view

export class Animation3D {
  readonly renderer: THREE.WebGLRenderer;
  readonly scene = new THREE.Scene();
  readonly camera = new THREE.PerspectiveCamera(40, 1, 0.01, 5000);
  readonly controls: OrbitControls;
  private root = new THREE.Group();
  private meshes: { s: AnimShape; m: THREE.Mesh; moving: boolean }[] = [];
  private data: AnimData | null = null;
  private ground: THREE.Mesh;
  /** Size of one floor tile (ten grid cells); the floor moves in whole tiles so its pattern stays put. */
  private tile = 1;
  private sun: THREE.DirectionalLight;
  private up = new THREE.Vector3(0, 0, 1);
  private bar: HTMLElement;
  private slider: HTMLInputElement;
  private label: HTMLElement;
  private playBtn: HTMLButtonElement;
  private speedSel: HTMLSelectElement;
  private followBox: HTMLInputElement;
  private canvasHost: HTMLElement;
  /** Current simulation time. */
  t = 0;
  playing = false;
  private last = 0;
  private followPrev: THREE.Vector3 | null = null;
  private raf = 0;

  constructor(readonly host: HTMLElement) {
    host.classList.add("anim3d");
    this.canvasHost = document.createElement("div");
    this.canvasHost.className = "anim-canvas";
    host.appendChild(this.canvasHost);
    this.renderer = new THREE.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true });
    this.renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    this.canvasHost.appendChild(this.renderer.domElement);
    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.addEventListener("change", () => this.render());

    this.scene.background = new THREE.Color(0xf3f5f8);
    this.scene.add(new THREE.HemisphereLight(0xffffff, 0x8a8f99, 1.6));
    this.sun = new THREE.DirectionalLight(0xffffff, 2.2);
    this.sun.castShadow = true;
    this.sun.shadow.mapSize.set(2048, 2048);
    this.scene.add(this.sun, this.sun.target);
    this.ground = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), new THREE.MeshStandardMaterial({ map: this.floorTexture(), roughness: 0.95 }));
    this.ground.receiveShadow = true;
    this.scene.add(this.ground, this.root);

    // playback bar
    this.bar = document.createElement("div");
    this.bar.className = "anim-bar";
    this.bar.innerHTML = `
      <button data-a="start" title="Rewind">⏮</button>
      <button data-a="play" title="Play / pause (space)">▶</button>
      <input type="range" min="0" max="1000" value="0" class="anim-slider" />
      <span class="anim-time">t = 0 s</span>
      <label>Speed <select class="anim-speed"><option>0.1</option><option>0.25</option><option>0.5</option><option selected>1</option><option>2</option><option>5</option></select>×</label>
      <label title="Keep the moving parts in view"><input type="checkbox" class="anim-follow" checked /> Follow</label>
      <button data-a="fit" title="Fit the view">Fit</button>
      <span class="spacer"></span>
      <span class="anim-info muted"></span>`;
    host.appendChild(this.bar);
    this.slider = this.bar.querySelector(".anim-slider")!;
    this.label = this.bar.querySelector(".anim-time")!;
    this.playBtn = this.bar.querySelector('[data-a="play"]')!;
    this.speedSel = this.bar.querySelector(".anim-speed")!;
    this.followBox = this.bar.querySelector(".anim-follow")!;
    this.bar.querySelector<HTMLButtonElement>('[data-a="start"]')!.onclick = () => this.seek(this.t0());
    this.playBtn.onclick = () => this.toggle();
    this.bar.querySelector<HTMLButtonElement>('[data-a="fit"]')!.onclick = () => this.fit();
    this.slider.oninput = () => {
      const tt = this.data?.time ?? [0];
      this.seek(this.t0() + ((tt[tt.length - 1] - this.t0()) * +this.slider.value) / 1000);
    };
    host.tabIndex = 0;
    host.addEventListener("keydown", (e) => {
      if (e.key === " ") {
        e.preventDefault();
        this.toggle();
      }
    });
    new ResizeObserver(() => this.resize()).observe(this.canvasHost);
  }

  /** One floor tile: ten grid cells with a darker border line. */
  private floorTexture() {
    const n = 512, c = document.createElement("canvas");
    c.width = c.height = n;
    const g = c.getContext("2d")!;
    g.fillStyle = "#e9edf2";
    g.fillRect(0, 0, n, n);
    for (let i = 0; i <= 10; i++) {
      const x = Math.round((i * n) / 10);
      g.fillStyle = i % 10 === 0 ? "#9aa4b2" : "#c8cfd8";
      const w = i % 10 === 0 ? 3 : 2;
      g.fillRect(x - w / 2, 0, w, n);
      g.fillRect(0, x - w / 2, n, w);
    }
    const t = new THREE.CanvasTexture(c);
    t.colorSpace = THREE.SRGBColorSpace;
    t.wrapS = t.wrapT = THREE.RepeatWrapping;
    t.anisotropy = 8;
    return t;
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
    for (const { m } of this.meshes) {
      (m.material as THREE.Material).dispose();
      this.root.remove(m);
    }
    this.meshes = [];
    const g = d.gravity.length === 3 && Math.hypot(d.gravity[0], d.gravity[1], d.gravity[2]) > 0 ? d.gravity : [0, -1, 0];
    this.up.set(-g[0], -g[1], -g[2]).normalize();
    this.camera.up.copy(this.up);
    for (const s of d.shapes) {
      const extra = at(s.extra, 0, 0);
      const wire = s.type === "spring" ? Math.min(0.45, Math.max(0.02, at(s.height, 0, 0) / Math.max(1e-9, at(s.width, 0, 0)))) : 0;
      const col = s.color.map((c) => Math.max(0, Math.min(255, at(c, 0, 0))) / 255);
      const spec = at(s.specularCoefficient, 0, 0);
      const mat = new THREE.MeshStandardMaterial({
        color: new THREE.Color().setRGB(col[0], col[1], col[2], THREE.SRGBColorSpace),
        roughness: Math.max(0.15, 0.9 - 0.5 * spec),
        metalness: 0.05,
        side: s.type === "pipe" ? THREE.DoubleSide : THREE.FrontSide,
      });
      const m = new THREE.Mesh(unitGeometry(s.type, extra, wire), mat);
      m.matrixAutoUpdate = false;
      m.castShadow = true;
      m.name = s.name;
      const moving = [...s.r, ...s.T, ...s.r_shape, ...s.lengthDirection, ...s.widthDirection, s.length, s.width, s.height].some((x) => typeof x !== "number");
      this.meshes.push({ s, m, moving });
      this.root.add(m);
    }
    const tt = d.time;
    const span = tt.length ? tt[tt.length - 1] - tt[0] : 0;
    this.bar.querySelector(".anim-info")!.textContent = `${d.shapes.length} shapes · ${d.model}`;
    this.t = tt[0] ?? 0;
    // A slow-motion default for short transients, real time otherwise.
    this.speedSel.value = span > 0 && span < 1 ? "0.1" : "1";
    this.place(this.t);
    this.fit();
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
    for (const { s, m } of this.meshes) {
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
      const L = val(s.length), W = val(s.width);
      // a spring's height is its wire diameter; the coil is round with diameter width
      const H = s.type === "spring" ? W : val(s.height);
      const cx = ex.clone().applyMatrix3(R).multiplyScalar(L);
      const cy = ey.clone().applyMatrix3(R).multiplyScalar(W);
      const cz = ez.clone().applyMatrix3(R).multiplyScalar(H);
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
    let n = 0;
    for (const { m, moving } of this.meshes) {
      if (!moving || !m.visible) continue;
      box.expandByPoint(new THREE.Vector3().setFromMatrixPosition(m.matrix));
      n++;
    }
    return n ? box.getCenter(new THREE.Vector3()) : null;
  }

  /** Frame the whole model (or, when following, the moving parts). */
  fit() {
    if (!this.data) return;
    this.root.updateMatrixWorld(true);
    const box = new THREE.Box3();
    for (const { m, moving } of this.meshes) if (m.visible && (moving || !this.followBox.checked || !this.meshes.some((x) => x.moving))) box.expandByObject(m);
    if (box.isEmpty()) for (const { m } of this.meshes) if (m.visible) box.expandByObject(m);
    if (box.isEmpty()) box.setFromCenterAndSize(new THREE.Vector3(), new THREE.Vector3(1, 1, 1));
    const c = box.getCenter(new THREE.Vector3());
    const size = box.getSize(new THREE.Vector3()).length() || 1;
    // A three-quarter front view, like the default in most multibody tools.
    const side = Math.abs(this.up.z) > 0.5 ? new THREE.Vector3(1.0, -0.9, 0) : new THREE.Vector3(1.0, 0, 0.9);
    side.addScaledVector(this.up, 0.55).normalize();
    this.camera.position.copy(c).addScaledVector(side, size * 1.25);
    this.camera.near = size / 200;
    this.camera.far = size * 200;
    this.camera.updateProjectionMatrix();
    this.controls.target.copy(c);
    this.controls.update();
    this.followPrev = this.movingCentre();
    this.layoutGround(box);
    this.render();
  }

  private layoutGround(box: THREE.Box3) {
    // ground plane just below the lowest point of the model, normal to "up"
    // (from the moving parts when there are any, so a road marking below the road does not sink it)
    let low = Infinity;
    const anyMoving = this.meshes.some((x) => x.moving);
    for (const { m, moving } of this.meshes) {
      if (!m.visible || (anyMoving && !moving)) continue;
      const b = new THREE.Box3().setFromObject(m);
      for (const x of [b.min, b.max]) low = Math.min(low, x.dot(this.up));
    }
    if (!isFinite(low)) low = 0;
    const diag = Math.max(0.5, box.getSize(new THREE.Vector3()).length());
    const size = diag * 24;
    const q = new THREE.Quaternion().setFromUnitVectors(new THREE.Vector3(0, 0, 1), this.up);
    const c = box.getCenter(new THREE.Vector3());
    const onPlane = c.clone().addScaledVector(this.up, low - c.dot(this.up) - 1e-3);
    this.ground.quaternion.copy(q);
    this.ground.position.copy(onPlane);
    // grid cells of 1, 10 or 100 mm, cm, m...: about a tenth of the model's size
    this.tile = 10 * Math.pow(10, Math.floor(Math.log10(diag / 2.5)));
    const tiles = Math.ceil(size / this.tile);
    this.ground.scale.set(tiles * this.tile, tiles * this.tile, 1);
    (this.ground.material as THREE.MeshStandardMaterial).map!.repeat.set(tiles, tiles);
    this.ground.position.copy(this.snap(onPlane));
    // fade the far floor into the background
    this.scene.fog = new THREE.Fog(0xf3f5f8, diag * 3, diag * 11);
    // sun above and to the side, shadow box around the model
    const r = box.getSize(new THREE.Vector3()).length() || 1;
    const dir = this.up.clone().multiplyScalar(3).add(new THREE.Vector3(0.5, 0.35, 0.25)).normalize();
    this.sun.position.copy(c).addScaledVector(dir, r * 3);
    this.sun.target.position.copy(c);
    const cam = this.sun.shadow.camera;
    cam.left = cam.bottom = -r;
    cam.right = cam.top = r;
    cam.near = 0.01;
    cam.far = r * 8;
    cam.updateProjectionMatrix();
  }

  private follow() {
    if (!this.followBox.checked) {
      this.followPrev = null;
      return;
    }
    const c = this.movingCentre();
    if (!c) return;
    if (this.followPrev) {
      const d = c.clone().sub(this.followPrev);
      this.camera.position.add(d);
      this.controls.target.add(d);
      this.sun.position.add(d);
      this.sun.target.position.add(d);
      // keep the floor under the model
      const h = this.ground.position.dot(this.up);
      const g = c.clone().addScaledVector(this.up, h - c.dot(this.up));
      this.ground.position.copy(this.snap(g));
    }
    this.followPrev = c;
  }

  /** Round a point on the floor to whole tiles, keeping its height. */
  private snap(p: THREE.Vector3) {
    const k = this.tile;
    const h = p.dot(this.up);
    const s = new THREE.Vector3(Math.round(p.x / k) * k, Math.round(p.y / k) * k, Math.round(p.z / k) * k);
    return s.addScaledVector(this.up, h - s.dot(this.up));
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
    const step = (now: number) => {
      if (!this.playing) return;
      const dt = ((now - this.last) / 1000) * +this.speedSel.value;
      this.last = now;
      const t1 = tt[tt.length - 1];
      this.seek(this.t + dt >= t1 ? t1 : this.t + dt);
      if (this.t >= t1) {
        this.stop();
        return;
      }
      this.raf = requestAnimationFrame(step);
    };
    this.raf = requestAnimationFrame(step);
  }

  stop() {
    this.playing = false;
    cancelAnimationFrame(this.raf);
    this.playBtn.textContent = "▶";
  }

  resize() {
    const w = this.canvasHost.clientWidth, h = this.canvasHost.clientHeight;
    if (!w || !h) return;
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.render();
  }

  render() {
    this.renderer.render(this.scene, this.camera);
  }
}
