export interface PaletteDefinition {
  key: string;
  name: string;
  background: string;
  fog: string;
  colors: [string, string, string, string];
  keyLight: string;
  rimLight: string;
  fillLight: string;
  nebula: [string, string, string];
  dust: string;
}

/** Палитры 3D-фона. Не заменяют дизайн-токены интерфейса. */
export const PALETTES: PaletteDefinition[] = [
  {
    key: 'neon',
    name: 'Неон',
    background: '#050b16',
    fog: '#0a0620',
    colors: ['#ff4fd8', '#8b5cf6', '#58d4ff', '#5aa2ff'],
    keyLight: '#ff4fd8',
    rimLight: '#58d4ff',
    fillLight: '#8b5cf6',
    nebula: ['#3b1d6e', '#0e3a5c', '#5c1440'],
    dust: '#9db8ff',
  },
  {
    key: 'aurora',
    name: 'Аврора',
    background: '#02100d',
    fog: '#04201a',
    colors: ['#3dffb0', '#58d4ff', '#8aff5e', '#5aa2ff'],
    keyLight: '#3dffb0',
    rimLight: '#58d4ff',
    fillLight: '#1f8f6f',
    nebula: ['#0c4a3a', '#0e3a5c', '#123a24'],
    dust: '#a8ffd9',
  },
  {
    key: 'magma',
    name: 'Магма',
    background: '#120404',
    fog: '#200a06',
    colors: ['#ff4d2e', '#ffb347', '#ff4fd8', '#ffd166'],
    keyLight: '#ff6a3d',
    rimLight: '#ffb347',
    fillLight: '#a3241f',
    nebula: ['#5c1408', '#4a2a0c', '#3a0c2a'],
    dust: '#ffc9a8',
  },
  {
    key: 'ice',
    name: 'Лёд',
    background: '#030a14',
    fog: '#061426',
    colors: ['#7cd6ff', '#5aa2ff', '#d6f4ff', '#8b9fff'],
    keyLight: '#7cd6ff',
    rimLight: '#5aa2ff',
    fillLight: '#2a4a8f',
    nebula: ['#0c2a5c', '#123a4a', '#1a2a6e'],
    dust: '#cfe8ff',
  },
  {
    key: 'acid',
    name: 'Кислота',
    background: '#080f02',
    fog: '#101c06',
    colors: ['#c8ff2e', '#3dffb0', '#ffe14d', '#58d4ff'],
    keyLight: '#c8ff2e',
    rimLight: '#3dffb0',
    fillLight: '#4a8f1f',
    nebula: ['#2a4a0c', '#0c3a2a', '#3a3a0c'],
    dust: '#e8ffb0',
  },
];

export function paletteByKey(key: string): PaletteDefinition {
  return PALETTES.find((palette) => palette.key === key) ?? PALETTES[0];
}
