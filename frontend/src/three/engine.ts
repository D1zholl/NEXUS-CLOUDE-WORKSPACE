import * as THREE from 'three';
import { EffectComposer } from 'three/examples/jsm/postprocessing/EffectComposer.js';
import { OutputPass } from 'three/examples/jsm/postprocessing/OutputPass.js';
import { RenderPass } from 'three/examples/jsm/postprocessing/RenderPass.js';
import { ShaderPass } from 'three/examples/jsm/postprocessing/ShaderPass.js';
import { UnrealBloomPass } from 'three/examples/jsm/postprocessing/UnrealBloomPass.js';
import { RoomEnvironment } from 'three/examples/jsm/environments/RoomEnvironment.js';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';

import type { BackgroundSettings } from '../services/api';
import { paletteByKey, type PaletteDefinition } from './palettes';

const TAU = Math.PI * 2;
const MAX_CUBES = 700;
const MAX_DUST = 2200;
const MAX_STARS = 2500;
const BASE_DEPTH = 14;
const BASE_Z = 34;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function rand(a = 1, b?: number): number {
  return b === undefined ? Math.random() * a : a + Math.random() * (b - a);
}

function smooth(edge0: number, edge1: number, x: number): number {
  const t = clamp((x - edge0) / (edge1 - edge0), 0, 1);
  return t * t * (3 - 2 * t);
}

function radialTexture(size: number, stops: Array<[number, number]>): THREE.CanvasTexture {
  const canvas = document.createElement('canvas');
  canvas.width = size;
  canvas.height = size;
  const context = canvas.getContext('2d');
  if (context) {
    const gradient = context.createRadialGradient(
      size / 2,
      size / 2,
      0,
      size / 2,
      size / 2,
      size / 2,
    );
    for (const [offset, alpha] of stops) {
      gradient.addColorStop(offset, `rgba(255,255,255,${alpha})`);
    }
    context.fillStyle = gradient;
    context.fillRect(0, 0, size, size);
  }
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  return texture;
}

function frameGeometry(thickness = 0.07): THREE.BufferGeometry {
  const half = 0.5;
  const length = 1 + thickness;
  const parts: THREE.BufferGeometry[] = [];
  for (const a of [-half, half]) {
    for (const b of [-half, half]) {
      const bar = new THREE.BoxGeometry(length, thickness, thickness);
      bar.translate(0, a, b);
      parts.push(bar);
    }
  }
  for (const a of [-half, half]) {
    for (const b of [-half, half]) {
      const bar = new THREE.BoxGeometry(thickness, length, thickness);
      bar.translate(a, 0, b);
      parts.push(bar);
    }
  }
  for (const a of [-half, half]) {
    for (const b of [-half, half]) {
      const bar = new THREE.BoxGeometry(thickness, thickness, length);
      bar.translate(a, b, 0);
      parts.push(bar);
    }
  }
  const merged = mergeGeometries(parts);
  parts.forEach((part) => part.dispose());
  return merged ?? new THREE.BoxGeometry(1.1, 1.1, 1.1);
}

function sampleStops(stops: THREE.Color[], t: number, out: THREE.Color): THREE.Color {
  const x = clamp(t, 0, 1) * (stops.length - 1);
  const index = Math.min(Math.floor(x), stops.length - 2);
  return out.copy(stops[index]).lerp(stops[index + 1], x - index);
}

interface Cube {
  nx: number;
  nz: number;
  y: number;
  velocity: number;
  sizeSeed: number;
  big: boolean;
  rx: number;
  ry: number;
  rz: number;
  swayFreq: number;
  swayPhase: number;
  swayAmp: number;
  pulsePhase: number;
  colorT: number;
  color: THREE.Color;
  coreColor: THREE.Color;
}

interface Nebula {
  material: THREE.SpriteMaterial;
  baseRotation: number;
  spin: number;
}

export interface EngineOptions {
  preview?: boolean;
}

/**
 * Единый движок космического фона.
 * Использует один WebGL-контекст на экземпляр; параметры приходят с сервера
 * и могут меняться без пересоздания сцены.
 */
export class BackgroundEngine {
  private readonly canvas: HTMLCanvasElement;
  private readonly preview: boolean;
  private readonly settings: BackgroundSettings;
  private currentSettings: BackgroundSettings;

  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene: THREE.Scene;
  private readonly camera: THREE.PerspectiveCamera;
  private readonly composer: EffectComposer;
  private readonly bloomPass: UnrealBloomPass;
  private readonly filmPass: ShaderPass;

  private coreGeometry!: THREE.BoxGeometry;
  private frameGeometry!: THREE.BufferGeometry;
  private coreMaterial!: THREE.MeshStandardMaterial;
  private frameMaterial!: THREE.MeshBasicMaterial;
  private coreInstances!: THREE.InstancedMesh;
  private frameInstances!: THREE.InstancedMesh;
  private environment: THREE.Texture | null = null;
  private softTexture!: THREE.CanvasTexture;
  private dotTexture!: THREE.CanvasTexture;

  private dustGeometry!: THREE.BufferGeometry;
  private dustPoints!: THREE.Points;
  private dustColors!: Float32Array;
  private dustBaseColors!: Float32Array;

  private starGeometry!: THREE.BufferGeometry;
  private starPoints!: THREE.Points;
  private starMaterial!: THREE.PointsMaterial;
  private starBaseColors = new Float32Array(MAX_STARS * 3);
  private starMix = new Float32Array(MAX_STARS);
  private starSeed = new Float32Array(MAX_STARS);
  private starPhase = new Float32Array(MAX_STARS);
  private starColorTick = 0;

  private ambientLight!: THREE.AmbientLight;
  private keyLight!: THREE.DirectionalLight;
  private rimLight!: THREE.DirectionalLight;

  private cubes: Cube[] = [];
  private nebulae: Nebula[] = [];

  private readonly dustX = new Float32Array(MAX_DUST);
  private readonly dustY = new Float32Array(MAX_DUST);
  private readonly dustZ = new Float32Array(MAX_DUST);
  private readonly dustVelocity = new Float32Array(MAX_DUST);
  private readonly dustPhase = new Float32Array(MAX_DUST);
  private readonly dustColorT = new Float32Array(MAX_DUST);

  private palette: PaletteDefinition;

  private raf = 0;
  private running = false;
  private disposed = false;
  private userActive = true;
  private inView = true;
  private lastT = 0;
  private lastRender = 0;
  private elapsed = 0;
  private intro = 0;
  private fps = 60;
  private fpsFrames = 0;
  private fpsTime = 0;
  private autoDpr: number;
  private width = 1;
  private height = 1;
  private visible = true;

  private pointerX = 0;
  private pointerY = 0;
  private smoothX = 0;
  private smoothY = 0;

  private readonly resizeObserver: ResizeObserver | null = null;
  private readonly intersectionObserver: IntersectionObserver | null = null;
  private readonly offset = new THREE.Vector3();
  private readonly target = new THREE.Vector3();
  private readonly matrix = new THREE.Matrix4();
  private readonly quaternion = new THREE.Quaternion();
  private readonly euler = new THREE.Euler();
  private readonly position = new THREE.Vector3();
  private readonly scale = new THREE.Vector3();
  private readonly color = new THREE.Color();

  constructor(
    canvas: HTMLCanvasElement,
    settings: BackgroundSettings,
    options: EngineOptions = {},
  ) {
    this.canvas = canvas;
    this.preview = options.preview ?? false;
    this.settings = settings;
    this.currentSettings = settings;
    this.palette = paletteByKey(settings.palette);

    const parentWidth = canvas.parentElement?.clientWidth || window.innerWidth;
    const parentHeight = canvas.parentElement?.clientHeight || window.innerHeight;
    this.width = Math.max(1, parentWidth);
    this.height = Math.max(1, parentHeight);
    this.autoDpr = Math.min(window.devicePixelRatio || 1, this.preview ? 1.5 : 2);

    this.renderer = new THREE.WebGLRenderer({
      canvas,
      antialias: !this.preview,
      powerPreference: 'high-performance',
      alpha: false,
    });
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = 1.12 + 0.08 * clamp(settings.intensity, 0, 2);
    this.renderer.setSize(this.width, this.height, false);

    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(this.palette.background);
    this.scene.fog = new THREE.FogExp2(this.palette.fog, 0.02);

    this.camera = new THREE.PerspectiveCamera(
      clamp(58 * settings.perspective, 35, 100),
      this.width / this.height,
      0.1,
      500,
    );
    this.camera.position.set(0, 0, 52);

    const pmrem = new THREE.PMREMGenerator(this.renderer);
    this.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    this.scene.environment = this.environment;
    pmrem.dispose();

    this.composer = new EffectComposer(this.renderer);
    this.composer.addPass(new RenderPass(this.scene, this.camera));

    this.bloomPass = new UnrealBloomPass(new THREE.Vector2(this.width, this.height), 1, 0.6, 0.14);
    this.composer.addPass(this.bloomPass);
    this.composer.addPass(new OutputPass());

    this.filmPass = new ShaderPass({
      uniforms: {
        tDiffuse: { value: null },
        uTime: { value: 0 },
        uVig: { value: settings.vignette },
        uGrain: { value: settings.grain },
        uAber: { value: settings.aberration },
        uDim: { value: 0.16 },
      },
      vertexShader: `
        varying vec2 vUv;
        void main() {
          vUv = uv;
          gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
        }
      `,
      fragmentShader: `
        uniform sampler2D tDiffuse;
        uniform float uTime;
        uniform float uVig;
        uniform float uGrain;
        uniform float uAber;
        uniform float uDim;
        varying vec2 vUv;
        float hash(vec2 p) {
          return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
        }
        void main() {
          vec2 d = vUv - 0.5;
          float r2 = dot(d, d);
          vec2 off = d * r2 * uAber * 0.05;
          vec3 col;
          col.r = texture2D(tDiffuse, vUv + off).r;
          col.g = texture2D(tDiffuse, vUv).g;
          col.b = texture2D(tDiffuse, vUv - off).b;
          col *= 1.0 - uVig * smoothstep(0.10, 0.62, r2);
          col *= 1.0 - uDim;
          float n = hash(vUv * vec2(1234.5, 987.6) + mod(uTime, 10.0) * vec2(17.0, 29.0));
          col += (n - 0.5) * uGrain * 0.11;
          gl_FragColor = vec4(max(col, 0.0), 1.0);
        }
      `,
    });
    this.composer.addPass(this.filmPass);

    this.buildScene();
    this.applySettings(settings);
    this.resize();

    if (typeof ResizeObserver !== 'undefined') {
      this.resizeObserver = new ResizeObserver(() => this.resize());
      const parent = canvas.parentElement;
      if (parent) this.resizeObserver.observe(parent);
    }
    if (this.preview && typeof IntersectionObserver !== 'undefined') {
      this.intersectionObserver = new IntersectionObserver((entries) => {
        this.inView = entries.some((entry) => entry.isIntersecting);
        this.syncRunning();
      });
      this.intersectionObserver.observe(canvas);
    }

    this.bindEvents();
    this.renderFrame(performance.now());
    this.start();
  }

  /* ------------------------------------------------------------------ сцена */

  private buildScene(): void {
    this.softTexture = radialTexture(128, [
      [0, 1],
      [0.25, 0.55],
      [0.6, 0.12],
      [1, 0],
    ]);
    this.dotTexture = radialTexture(64, [
      [0, 1],
      [0.3, 0.8],
      [0.5, 0.15],
      [1, 0],
    ]);

    this.ambientLight = new THREE.AmbientLight(this.palette.fillLight, 0.55);
    this.keyLight = new THREE.DirectionalLight(this.palette.keyLight, 2.1);
    this.keyLight.position.set(7, 9, 8);
    this.rimLight = new THREE.DirectionalLight(this.palette.rimLight, 1.7);
    this.rimLight.position.set(-9, -6, -5);
    this.scene.add(this.ambientLight, this.keyLight, this.rimLight);

    this.coreGeometry = new THREE.BoxGeometry(0.9, 0.9, 0.9);
    this.frameGeometry = frameGeometry();
    this.coreMaterial = new THREE.MeshStandardMaterial({
      color: 0xffffff,
      metalness: 0.86,
      roughness: 0.27,
      envMapIntensity: 0.5,
      emissive: this.palette.fillLight,
      emissiveIntensity: 0.2,
      transparent: false,
    });
    this.frameMaterial = new THREE.MeshBasicMaterial({
      toneMapped: false,
      transparent: true,
      opacity: 0.62,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    this.coreInstances = new THREE.InstancedMesh(this.coreGeometry, this.coreMaterial, MAX_CUBES);
    this.frameInstances = new THREE.InstancedMesh(
      this.frameGeometry,
      this.frameMaterial,
      MAX_CUBES,
    );
    this.coreInstances.frustumCulled = false;
    this.frameInstances.frustumCulled = false;
    this.coreInstances.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    this.frameInstances.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    this.scene.add(this.coreInstances, this.frameInstances);

    for (let i = 0; i < MAX_CUBES; i += 1) {
      this.cubes.push({
        nx: rand(-1, 1),
        nz: rand(-1, 1),
        y: rand(-24, 24),
        velocity: rand(3, 9) * (Math.random() < 0.12 ? 0.75 : 1),
        sizeSeed: Math.random(),
        big: Math.random() < 0.12,
        rx: rand(-0.8, 0.8),
        ry: rand(-0.8, 0.8),
        rz: rand(-0.8, 0.8),
        swayFreq: rand(0.25, 0.8),
        swayPhase: rand(TAU),
        swayAmp: rand(0.02, 0.09),
        pulsePhase: rand(TAU),
        colorT: Math.random(),
        color: new THREE.Color(),
        coreColor: new THREE.Color(),
      });
    }

    /* Пыль */
    const dustPositions = new Float32Array(MAX_DUST * 3);
    this.dustColors = new Float32Array(MAX_DUST * 3);
    this.dustBaseColors = new Float32Array(MAX_DUST * 3);
    for (let i = 0; i < MAX_DUST; i += 1) {
      this.dustX[i] = rand(-1, 1);
      this.dustZ[i] = rand(-1, 1);
      this.dustY[i] = rand(-26, 26);
      this.dustVelocity[i] = rand(1.2, 4.5);
      this.dustPhase[i] = rand(TAU);
      this.dustColorT[i] = Math.random();
    }
    this.dustGeometry = new THREE.BufferGeometry();
    this.dustGeometry.setAttribute('position', new THREE.BufferAttribute(dustPositions, 3));
    this.dustGeometry.setAttribute('color', new THREE.BufferAttribute(this.dustColors, 3));
    this.dustPoints = new THREE.Points(
      this.dustGeometry,
      new THREE.PointsMaterial({
        size: 0.5,
        map: this.dotTexture,
        transparent: true,
        opacity: 0.7,
        vertexColors: true,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
        sizeAttenuation: true,
      }),
    );
    this.dustPoints.frustumCulled = false;
    this.scene.add(this.dustPoints);

    /* Звёзды */
    const starPositions = new Float32Array(MAX_STARS * 3);
    const starColors = new Float32Array(MAX_STARS * 3);
    for (let i = 0; i < MAX_STARS; i += 1) {
      const vector = new THREE.Vector3(rand(-1, 1), rand(-1, 1), rand(-1, 1))
        .normalize()
        .multiplyScalar(rand(170, 240));
      starPositions[i * 3] = vector.x;
      starPositions[i * 3 + 1] = vector.y;
      starPositions[i * 3 + 2] = vector.z;
      this.starMix[i] = Math.random();
      this.starSeed[i] = rand(0.35, 1);
      this.starPhase[i] = rand(TAU);
    }
    this.starGeometry = new THREE.BufferGeometry();
    this.starGeometry.setAttribute('position', new THREE.BufferAttribute(starPositions, 3));
    this.starGeometry.setAttribute('color', new THREE.BufferAttribute(starColors, 3));
    this.starMaterial = new THREE.PointsMaterial({
      size: 1.5,
      map: this.dotTexture,
      transparent: true,
      opacity: 0.8,
      vertexColors: true,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
      fog: false,
    });
    this.starPoints = new THREE.Points(this.starGeometry, this.starMaterial);
    this.starPoints.frustumCulled = false;
    this.scene.add(this.starPoints);

    /* Небулы */
    for (let i = 0; i < 5; i += 1) {
      const material = new THREE.SpriteMaterial({
        map: this.softTexture,
        color: this.palette.nebula[i % this.palette.nebula.length],
        transparent: true,
        opacity: 0.04,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
        fog: false,
        rotation: rand(TAU),
      });
      const sprite = new THREE.Sprite(material);
      sprite.position
        .set(rand(-1, 1), rand(-0.7, 0.7), rand(-1, -0.2))
        .normalize()
        .multiplyScalar(rand(60, 95));
      sprite.scale.setScalar(rand(65, 125));
      this.scene.add(sprite);
      this.nebulae.push({ material, baseRotation: material.rotation, spin: rand(-0.015, 0.015) });
    }

    this.assignColors();
  }

  /* --------------------------------------------------------------- цвета */

  private assignColors(): void {
    const palette = this.palette;
    const stops = palette.colors.map((value) => new THREE.Color(value));
    const dustTint = new THREE.Color(palette.dust);

    for (let i = 0; i < MAX_CUBES; i += 1) {
      const cube = this.cubes[i];
      sampleStops(stops, cube.colorT, cube.color);
      cube.coreColor.copy(cube.color).multiplyScalar(0.07);
      this.coreInstances.setColorAt(i, cube.coreColor);
    }
    if (this.coreInstances.instanceColor) this.coreInstances.instanceColor.needsUpdate = true;

    const dustBuffer = this.dustGeometry.getAttribute('color') as THREE.BufferAttribute | undefined;
    if (dustBuffer) {
      for (let i = 0; i < MAX_DUST; i += 1) {
        sampleStops(stops, this.dustColorT[i], this.color).lerp(dustTint, 0.35);
        this.dustBaseColors[i * 3] = this.color.r;
        this.dustBaseColors[i * 3 + 1] = this.color.g;
        this.dustBaseColors[i * 3 + 2] = this.color.b;
        this.dustColors[i * 3] = this.color.r;
        this.dustColors[i * 3 + 1] = this.color.g;
        this.dustColors[i * 3 + 2] = this.color.b;
      }
      dustBuffer.needsUpdate = true;
    }

    const white = new THREE.Color('#ffffff');
    const starTint = new THREE.Color(palette.dust);
    const starBuffer = this.starGeometry.getAttribute('color') as THREE.BufferAttribute | undefined;
    if (starBuffer) {
      for (let i = 0; i < MAX_STARS; i += 1) {
        this.color.copy(white).lerp(starTint, this.starMix[i]);
        const factor = this.starSeed[i] * 0.9;
        this.starBaseColors[i * 3] = this.color.r * factor;
        this.starBaseColors[i * 3 + 1] = this.color.g * factor;
        this.starBaseColors[i * 3 + 2] = this.color.b * factor;
      }
    }
    this.updateStarColors(this.currentSettings.starBrightness, this.currentSettings.twinkle, true);
  }

  private applyPalette(): void {
    const palette = this.palette;
    if (this.scene.background instanceof THREE.Color) {
      this.scene.background.set(palette.background);
    }
    if (this.scene.fog instanceof THREE.FogExp2) {
      this.scene.fog.color.set(palette.fog);
    }
    this.ambientLight.color.set(palette.fillLight);
    this.keyLight.color.set(palette.keyLight);
    this.rimLight.color.set(palette.rimLight);
    this.coreMaterial.emissive.set(palette.fillLight);
    this.nebulae.forEach((nebula, index) => {
      nebula.material.color.set(palette.nebula[index % palette.nebula.length]);
    });
    this.assignColors();
  }

  /* ------------------------------------------------------------- настройки */

  setSettings(next: BackgroundSettings): void {
    if (this.disposed) return;
    const paletteChanged = next.palette !== this.currentSettings.palette;
    this.currentSettings = next;
    if (paletteChanged) {
      // Палитра кешировалась в конструкторе: без обновления ссылки
      // `applyPalette()` перекрашивал сцену старыми цветами, поэтому смена
      // палитры (в том числе системный default на форме входа) не была видна
      // до пересоздания канваса — например, после переключения вкладки.
      this.palette = paletteByKey(next.palette);
      this.applyPalette();
    }
    this.applySettings(next);
  }

  private applySettings(settings: BackgroundSettings): void {
    const count = clamp(Math.round(settings.count), 40, MAX_CUBES);
    this.coreInstances.count = count;
    this.frameInstances.count = count;
    this.coreInstances.visible = !settings.wireframe;
    this.frameMaterial.opacity = clamp(settings.opacity, 0.05, 1);

    this.bloomPass.strength = clamp(settings.glow * settings.intensity * 0.55, 0, 3);
    this.renderer.toneMappingExposure = 1.05 + 0.1 * clamp(settings.intensity, 0, 2);
    if (this.scene.fog instanceof THREE.FogExp2) {
      this.scene.fog.density = clamp(
        0.018 + (1.2 - clamp(settings.intensity, 0, 2)) * 0.012,
        0.006,
        0.04,
      );
    }

    this.camera.fov = clamp(58 * settings.perspective, 35, 100);
    this.camera.updateProjectionMatrix();

    this.filmPass.uniforms.uVig.value = settings.vignette;
    this.filmPass.uniforms.uGrain.value = settings.grain;
    this.filmPass.uniforms.uAber.value = settings.aberration;

    const starCount = clamp(Math.round(settings.stars), 0, MAX_STARS);
    this.starGeometry.setDrawRange(0, starCount);
    this.starPoints.visible = starCount > 0;
    this.starMaterial.opacity = clamp(settings.starBrightness, 0, 1);
    this.starMaterial.size = clamp(settings.starSize * 3, 0.2, 4);
    this.starPoints.scale.setScalar(clamp(settings.starDepth, 0.2, 2));

    const dustCount = clamp(Math.round(settings.stars * 0.85), 0, MAX_DUST);
    this.dustGeometry.setDrawRange(0, dustCount);
    this.dustPoints.visible = dustCount > 0;
    (this.dustPoints.material as THREE.Material).opacity = clamp(
      0.35 + settings.starBrightness * 0.45,
      0.1,
      0.9,
    );

    this.ambientLight.intensity = 0.35 + 0.25 * clamp(settings.intensity, 0, 2);
    this.keyLight.intensity = 1.4 * clamp(settings.intensity, 0.2, 2);
    this.rimLight.intensity = 1.1 * clamp(settings.intensity, 0.2, 2);

    this.nebulae.forEach((nebula) => {
      nebula.material.opacity = clamp(0.02 + 0.02 * settings.intensity, 0.02, 0.06);
    });
    this.applyDpr();
    this.updateStarColors(settings.starBrightness, settings.twinkle, true);
    if (!this.running) {
      this.renderFrame(performance.now());
    }
  }

  private updateStarColors(brightness: number, twinkle: number, force = false): void {
    const count = clamp(Math.round(this.currentSettings.stars), 0, MAX_STARS);
    if (count === 0) return;
    this.starColorTick += 1;
    if (!force && this.starColorTick % 2 !== 0) return;
    const attribute = this.starGeometry.getAttribute('color') as THREE.BufferAttribute | undefined;
    if (!attribute) return;
    const array = attribute.array as Float32Array;
    for (let i = 0; i < count; i += 1) {
      const shimmer =
        1 - twinkle * 0.45 * (0.5 + 0.5 * Math.sin(this.elapsed * 1.6 + this.starPhase[i]));
      const factor = clamp(brightness, 0, 1) * shimmer;
      array[i * 3] = this.starBaseColors[i * 3] * factor;
      array[i * 3 + 1] = this.starBaseColors[i * 3 + 1] * factor;
      array[i * 3 + 2] = this.starBaseColors[i * 3 + 2] * factor;
    }
    attribute.needsUpdate = true;
  }

  /* ------------------------------------------------------------ управление */

  setActive(active: boolean): void {
    this.userActive = active;
    this.syncRunning();
  }

  setVisible(visible: boolean): void {
    this.visible = visible;
    if (!visible) {
      this.userActive = false;
    }
    this.syncRunning();
  }

  private syncRunning(): void {
    const shouldRun =
      this.userActive &&
      this.inView &&
      this.visible &&
      !this.disposed &&
      document.visibilityState !== 'hidden';
    if (shouldRun && !this.running) {
      this.running = true;
      this.lastT = 0;
      this.raf = requestAnimationFrame(this.frame);
    } else if (!shouldRun && this.running) {
      this.running = false;
      cancelAnimationFrame(this.raf);
      this.raf = 0;
    }
  }

  private start(): void {
    this.running = false;
    this.syncRunning();
  }

  /* ---------------------------------------------------------------- события */

  private bindEvents(): void {
    const target = this.preview ? this.canvas : window;
    target.addEventListener('pointermove', this.onPointerMove as EventListener, { passive: true });
    target.addEventListener('pointerleave', this.onPointerLeave as EventListener);
    window.addEventListener('resize', this.onWindowResize);
    document.addEventListener('visibilitychange', this.onVisibilityChange);
    this.canvas.addEventListener('webglcontextlost', this.onContextLost);
    this.canvas.addEventListener('webglcontextrestored', this.onContextRestored);
  }

  private readonly onPointerMove = (event: PointerEvent): void => {
    const width = this.preview ? this.canvas.clientWidth || 1 : window.innerWidth;
    const height = this.preview ? this.canvas.clientHeight || 1 : window.innerHeight;
    const rect = this.preview ? this.canvas.getBoundingClientRect() : null;
    const x = rect ? event.clientX - rect.left : event.clientX;
    const y = rect ? event.clientY - rect.top : event.clientY;
    this.pointerX = (x / width) * 2 - 1;
    this.pointerY = (y / height) * 2 - 1;
  };

  private readonly onPointerLeave = (): void => {
    this.pointerX = 0;
    this.pointerY = 0;
  };

  private readonly onWindowResize = (): void => {
    this.resize();
  };

  private readonly onVisibilityChange = (): void => {
    this.syncRunning();
  };

  private readonly onContextLost = (event: Event): void => {
    event.preventDefault();
    this.running = false;
    cancelAnimationFrame(this.raf);
  };

  private readonly onContextRestored = (): void => {
    this.resize();
    this.syncRunning();
  };

  private resize(): void {
    if (this.disposed) return;
    const parent = this.canvas.parentElement;
    const width = Math.max(1, parent?.clientWidth || window.innerWidth);
    const height = Math.max(1, parent?.clientHeight || window.innerHeight);
    if (
      width === this.width &&
      height === this.height &&
      this.renderer.getPixelRatio() === this.targetDpr()
    ) {
      return;
    }
    this.width = width;
    this.height = height;
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(width, height, false);
    this.applyDpr();
  }

  private targetDpr(): number {
    const cap = Math.min(window.devicePixelRatio || 1, this.preview ? 1.5 : 2);
    if (this.currentSettings.quality === 'eco') return 1;
    if (this.currentSettings.quality === 'high') return cap;
    return clamp(this.autoDpr, 1, cap);
  }

  private applyDpr(): void {
    const dpr = this.targetDpr();
    this.renderer.setPixelRatio(dpr);
    this.composer.setPixelRatio(dpr);
    this.composer.setSize(this.width, this.height);
  }

  /* ----------------------------------------------------------------- кадры */

  private readonly frame = (now: number): void => {
    if (!this.running) return;
    this.raf = requestAnimationFrame(this.frame);

    const fpsLimit = this.currentSettings.fpsLimit;
    const interval = fpsLimit > 0 ? 1000 / fpsLimit : 0;
    if (interval > 0 && now - this.lastRender < interval - 1) {
      return;
    }

    const dt = this.lastT === 0 ? 0.016 : Math.min((now - this.lastT) / 1000, 0.05);
    this.lastT = now;
    this.lastRender = now;
    this.elapsed += dt;

    this.updateCamera(dt);
    this.updateField(dt);
    this.updateDust(dt);
    this.updateAmbientMotion(dt);
    this.updateStarColors(this.currentSettings.starBrightness, this.currentSettings.twinkle);

    this.filmPass.uniforms.uTime.value = this.elapsed;
    this.composer.render();
    this.trackFps(dt, now);
  };

  private updateCamera(dt: number): void {
    const settings = this.currentSettings;
    this.intro = Math.min(1, this.intro + dt / 2.2);
    const eased = 1 - Math.pow(1 - this.intro, 3);
    const baseZ = BASE_Z + (52 - BASE_Z) * (1 - eased);

    const smoothFactor = Math.min(1, dt * 3);
    this.smoothX += (this.pointerX - this.smoothX) * smoothFactor;
    this.smoothY += (this.pointerY - this.smoothY) * smoothFactor;

    const sensitivity = 0.35 + 0.65 * clamp(settings.mouseSensitivity, 0, 1);
    const parallax = clamp(settings.parallax, 0, 1) * sensitivity;
    const drift = clamp(settings.cameraAmplitude, 0, 1);

    this.offset.set(
      this.smoothX * parallax * 2.4 + Math.sin(this.elapsed * 0.09) * drift * 1.8,
      -this.smoothY * parallax * 1.5 + Math.cos(this.elapsed * 0.073) * drift * 1.2,
      Math.sin(this.elapsed * 0.05) * drift * 0.8,
    );
    this.camera.position.set(this.offset.x, this.offset.y, baseZ + this.offset.z);
    this.target.set(this.offset.x * 0.5, this.offset.y * 0.5, 0);
    this.camera.lookAt(this.target);
  }

  private updateField(dt: number): void {
    const settings = this.currentSettings;
    const count = clamp(Math.round(settings.count), 40, MAX_CUBES);
    const aspect = this.camera.aspect;
    const tangent = Math.tan((this.camera.fov * Math.PI) / 360);
    const cameraDistance = this.camera.position.length();
    const speed = clamp(settings.speed, 0, 3);
    const rotation = clamp(settings.rotation, 0, 3);
    const depth = clamp(settings.depth, 0.2, 2);
    const spread = clamp(settings.spread, 0.2, 2);
    const spacing = clamp(settings.spacing, 0.2, 2.5);
    const sizeMin = Math.min(settings.sizeMin, settings.sizeMax);
    const sizeMax = Math.max(settings.sizeMin, settings.sizeMax);
    const intensity = clamp(settings.intensity, 0, 2);

    for (let i = 0; i < count; i += 1) {
      const cube = this.cubes[i];
      const z = Math.min(cube.nz * BASE_DEPTH * depth * spacing, cameraDistance - 6);
      const distance = Math.max(cameraDistance - z, 4);
      const halfHeight = tangent * distance;
      const height = halfHeight * 1.35;
      const width = halfHeight * aspect * 1.12;

      let y = cube.y + dt * cube.velocity * speed;
      if (y > height || y < -height) {
        y = ((((y + height) % (2 * height)) + 2 * height) % (2 * height)) - height;
      }
      cube.y = y;

      const fade = 1 - smooth(0.72, 0.98, Math.abs(y) / height);
      const x =
        cube.nx * width * spread +
        Math.sin(this.elapsed * cube.swayFreq + cube.swayPhase) * 0.7 * cube.swayAmp * width;
      const baseSize = sizeMin + Math.pow(cube.sizeSeed, 1.7) * (sizeMax - sizeMin);
      const scale = Math.max(0.01, baseSize * (cube.big ? 2.1 : 1) * fade);

      this.position.set(x, y, z);
      this.euler.set(
        this.elapsed * rotation * cube.rx + cube.swayPhase,
        this.elapsed * rotation * cube.ry,
        this.elapsed * rotation * cube.rz,
      );
      this.quaternion.setFromEuler(this.euler);
      this.scale.set(scale, scale * (1 + Math.min(speed * cube.velocity * 0.012, 0.45)), scale);
      this.matrix.compose(this.position, this.quaternion, this.scale);
      this.coreInstances.setMatrixAt(i, this.matrix);
      this.frameInstances.setMatrixAt(i, this.matrix);

      this.color
        .copy(cube.color)
        .multiplyScalar(
          (1.05 + 0.45 * (0.5 + 0.5 * Math.sin(this.elapsed * 1.8 + cube.pulsePhase))) *
            fade *
            (0.8 + intensity * 0.35),
        );
      this.frameInstances.setColorAt(i, this.color);
    }

    this.coreInstances.instanceMatrix.needsUpdate = true;
    this.frameInstances.instanceMatrix.needsUpdate = true;
    if (this.frameInstances.instanceColor) {
      this.frameInstances.instanceColor.needsUpdate = true;
    }
  }

  private updateDust(dt: number): void {
    const settings = this.currentSettings;
    const count = clamp(Math.round(settings.stars * 0.85), 0, MAX_DUST);
    if (count === 0) return;
    const aspect = this.camera.aspect;
    const tangent = Math.tan((this.camera.fov * Math.PI) / 360);
    const cameraDistance = this.camera.position.length();
    const speed = clamp(settings.speed, 0, 3);
    const depth = clamp(settings.depth, 0.2, 2);
    const spacing = clamp(settings.spacing, 0.2, 2.5);
    const spread = clamp(settings.spread, 0.2, 2);
    const attribute = this.dustGeometry.getAttribute('position') as THREE.BufferAttribute;

    for (let i = 0; i < count; i += 1) {
      const z = Math.min(this.dustZ[i] * BASE_DEPTH * depth * spacing, cameraDistance - 6);
      const distance = Math.max(cameraDistance - z, 4);
      const halfHeight = tangent * distance;
      const height = halfHeight * 1.35;
      const width = halfHeight * aspect * 1.12;

      let y = this.dustY[i] + dt * this.dustVelocity[i] * speed;
      if (y > height || y < -height) {
        y = ((((y + height) % (2 * height)) + 2 * height) % (2 * height)) - height;
      }
      this.dustY[i] = y;
      attribute.setXYZ(
        i,
        this.dustX[i] * width * spread +
          Math.sin(this.elapsed * 0.35 + this.dustPhase[i]) * 0.05 * width,
        y,
        z,
      );
    }
    attribute.needsUpdate = true;

    const colorAttribute = this.dustGeometry.getAttribute('color') as THREE.BufferAttribute;
    const pulse = 0.85 + 0.15 * Math.sin(this.elapsed * 0.8);
    for (let i = 0; i < count; i += 1) {
      colorAttribute.setXYZ(
        i,
        this.dustBaseColors[i * 3] * pulse,
        this.dustBaseColors[i * 3 + 1] * pulse,
        this.dustBaseColors[i * 3 + 2] * pulse,
      );
    }
    colorAttribute.needsUpdate = true;
  }

  private updateAmbientMotion(_dt: number): void {
    const settings = this.currentSettings;
    const speed = clamp(settings.backgroundSpeed, 0, 3);
    this.starPoints.rotation.y = -this.elapsed * 0.004 * speed;
    this.nebulae.forEach((nebula) => {
      nebula.material.rotation = nebula.baseRotation + this.elapsed * nebula.spin * speed;
    });
  }

  private trackFps(dt: number, now: number): void {
    this.fpsFrames += 1;
    this.fpsTime += dt;
    if (this.fpsTime < 0.6) return;
    this.fps = this.fpsFrames / this.fpsTime;
    this.fpsFrames = 0;
    this.fpsTime = 0;

    if (this.currentSettings.quality !== 'auto' || this.preview) return;
    const cap = Math.min(window.devicePixelRatio || 1, 2);
    if (this.fps < 38 && this.autoDpr > 1) {
      this.autoDpr = Math.max(1, this.autoDpr - 0.25);
      this.applyDpr();
    } else if (this.fps > 57 && this.autoDpr < cap && now - this.lastRender > 0) {
      this.autoDpr = Math.min(cap, this.autoDpr + 0.25);
      this.applyDpr();
    }
  }

  private renderFrame(now: number): void {
    this.filmPass.uniforms.uTime.value = now * 0.001;
    this.composer.render();
  }

  /* ---------------------------------------------------------------- очистка */

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.running = false;
    cancelAnimationFrame(this.raf);

    const target = this.preview ? this.canvas : window;
    target.removeEventListener('pointermove', this.onPointerMove as EventListener);
    target.removeEventListener('pointerleave', this.onPointerLeave as EventListener);
    window.removeEventListener('resize', this.onWindowResize);
    document.removeEventListener('visibilitychange', this.onVisibilityChange);
    this.canvas.removeEventListener('webglcontextlost', this.onContextLost);
    this.canvas.removeEventListener('webglcontextrestored', this.onContextRestored);
    this.resizeObserver?.disconnect();
    this.intersectionObserver?.disconnect();

    this.coreGeometry.dispose();
    this.frameGeometry.dispose();
    this.coreMaterial.dispose();
    this.frameMaterial.dispose();
    this.dustGeometry.dispose();
    (this.dustPoints.material as THREE.Material).dispose();
    this.dotTexture.dispose();
    this.softTexture.dispose();
    this.starGeometry.dispose();
    this.starMaterial.dispose();
    this.nebulae.forEach((nebula) => nebula.material.dispose());
    this.environment?.dispose();
    this.composer.dispose();
    const context = this.renderer.getContext();
    context.pixelStorei(context.UNPACK_FLIP_Y_WEBGL, false);
    context.pixelStorei(context.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    this.renderer.dispose();
  }
}