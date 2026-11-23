import type { BackgroundSettings } from '../services/api';

export const DEFAULT_BACKGROUND: BackgroundSettings = {
  enabled: true,
  preset: 'flow',
  palette: 'neon',
  intensity: 1,
  quality: 'auto',
  fpsLimit: 0,
  count: 260,
  sizeMin: 0.35,
  sizeMax: 1.45,
  speed: 1,
  rotation: 1,
  opacity: 0.62,
  glow: 0.9,
  spacing: 1,
  depth: 1,
  spread: 1,
  wireframe: false,
  stars: 1200,
  starBrightness: 0.8,
  twinkle: 0.5,
  starSize: 0.5,
  starDepth: 1,
  backgroundSpeed: 1,
  parallax: 0.5,
  mouseSensitivity: 0.5,
  perspective: 1,
  cameraAmplitude: 0.4,
  vignette: 0.6,
  grain: 0.3,
  aberration: 0.25,
};

export type PresetKey = 'flow' | 'aurora' | 'magma' | 'ghost';

export const BACKGROUND_PRESETS: Record<
  PresetKey,
  { name: string; patch: Partial<BackgroundSettings> }
> = {
  flow: {
    name: 'Поток',
    patch: {
      palette: 'neon',
      intensity: 1.2,
      count: 260,
      sizeMin: 0.5,
      sizeMax: 1.65,
      speed: 1,
      rotation: 1,
      opacity: 0.62,
      glow: 1.1,
      spacing: 1,
      depth: 1,
      spread: 1,
      wireframe: false,
      stars: 1200,
      starBrightness: 0.8,
      twinkle: 0.5,
      starSize: 0.5,
      starDepth: 1,
      backgroundSpeed: 1,
      parallax: 0.5,
      mouseSensitivity: 0.5,
      perspective: 1,
      cameraAmplitude: 0.4,
      vignette: 0.6,
      grain: 0.3,
      aberration: 0.25,
    },
  },
  aurora: {
    name: 'Аврора',
    patch: {
      palette: 'aurora',
      intensity: 1.05,
      count: 340,
      sizeMin: 0.45,
      sizeMax: 1.5,
      speed: 0.6,
      rotation: 0.7,
      opacity: 0.58,
      glow: 1,
      spacing: 1.1,
      depth: 1.3,
      spread: 1.2,
      wireframe: false,
      stars: 2000,
      starBrightness: 0.85,
      twinkle: 0.7,
      starSize: 0.6,
      starDepth: 1.1,
      backgroundSpeed: 0.6,
      parallax: 0.6,
      mouseSensitivity: 0.55,
      perspective: 1.1,
      cameraAmplitude: 0.3,
      vignette: 0.55,
      grain: 0.22,
      aberration: 0.18,
    },
  },
  magma: {
    name: 'Магма',
    patch: {
      palette: 'magma',
      intensity: 1.45,
      count: 200,
      sizeMin: 0.7,
      sizeMax: 2.4,
      speed: 1.8,
      rotation: 1.6,
      opacity: 0.7,
      glow: 1.7,
      spacing: 0.9,
      depth: 0.8,
      spread: 0.8,
      wireframe: false,
      stars: 900,
      starBrightness: 0.6,
      twinkle: 0.25,
      starSize: 0.5,
      starDepth: 0.9,
      backgroundSpeed: 1.6,
      parallax: 0.45,
      mouseSensitivity: 0.5,
      perspective: 1.25,
      cameraAmplitude: 0.55,
      vignette: 0.7,
      grain: 0.4,
      aberration: 0.45,
    },
  },
  ghost: {
    name: 'Призрак',
    patch: {
      palette: 'ice',
      intensity: 1.1,
      count: 180,
      sizeMin: 0.8,
      sizeMax: 2.6,
      speed: 0.45,
      rotation: 0.5,
      opacity: 0.5,
      glow: 1.35,
      spacing: 1.3,
      depth: 1.1,
      spread: 1.1,
      wireframe: true,
      stars: 700,
      starBrightness: 0.5,
      twinkle: 0.2,
      starSize: 0.4,
      starDepth: 0.8,
      backgroundSpeed: 0.25,
      parallax: 0.35,
      mouseSensitivity: 0.4,
      perspective: 0.85,
      cameraAmplitude: 0.25,
      vignette: 0.78,
      grain: 0.45,
      aberration: 0.12,
    },
  },
};

export const PRESET_ORDER: PresetKey[] = ['flow', 'aurora', 'magma', 'ghost'];

export const PALETTE_SWATCHES: Array<{ key: string; name: string; gradient: string }> = [
  { key: 'neon', name: 'Неон', gradient: 'linear-gradient(120deg, #ff4fd8, #8b5cf6, #58d4ff)' },
  { key: 'aurora', name: 'Аврора', gradient: 'linear-gradient(120deg, #3dffb0, #58d4ff, #8aff5e)' },
  { key: 'magma', name: 'Магма', gradient: 'linear-gradient(120deg, #ff4d2e, #ffb347, #ff2ea6)' },
  { key: 'ice', name: 'Лёд', gradient: 'linear-gradient(120deg, #7cd6ff, #5aa2ff, #d6f4ff)' },
  { key: 'acid', name: 'Кислота', gradient: 'linear-gradient(120deg, #c8ff2e, #3dffb0, #ffe14d)' },
];

export function settingsEqual(a: BackgroundSettings, b: BackgroundSettings): boolean {
  const keys = Object.keys(DEFAULT_BACKGROUND) as Array<keyof BackgroundSettings>;
  return keys.every((key) => a[key] === b[key]);
}

export function sanitizeBackground(value: unknown): BackgroundSettings {
  if (typeof value !== 'object' || value === null) return { ...DEFAULT_BACKGROUND };
  const source = value as Record<string, unknown>;
  const result: BackgroundSettings = { ...DEFAULT_BACKGROUND };
  for (const key of Object.keys(DEFAULT_BACKGROUND) as Array<keyof BackgroundSettings>) {
    const fallback = DEFAULT_BACKGROUND[key];
    const raw = source[key];
    if (typeof fallback === 'number') {
      const numeric = typeof raw === 'number' ? raw : Number(raw);
      if (Number.isFinite(numeric)) {
        (result[key] as number) = numeric;
      }
    } else if (typeof fallback === 'boolean') {
      if (typeof raw === 'boolean') (result[key] as boolean) = raw;
    } else if (typeof raw === 'string') {
      (result[key] as string) = raw;
    }
  }
  return result;
}
