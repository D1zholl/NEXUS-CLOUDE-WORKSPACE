/**
 * Правила предпросмотра Cloud Storage.
 *
 * Модуль не загружает PDF.js и highlight.js: он только определяет тип
 * просмотрщика и лимиты. Тяжёлые библиотеки подключаются динамически внутри
 * соответствующих viewer-компонентов.
 */

export type PreviewKind = 'image' | 'video' | 'audio' | 'pdf' | 'code' | 'text' | 'none';

/** Общий лимит предпросмотра (PDF, медиа и документы). */
export const PREVIEW_MAX_BYTES = 100 * 1024 * 1024;

/** Лимит текстового предпросмотра: больше не подсвечиваем и не показываем. */
export const PREVIEW_TEXT_MAX_BYTES = 512 * 1024;

const IMAGE_RE = /\.(png|jpe?g|webp|gif|bmp|avif)$/i;
const VIDEO_RE = /\.(mp4|webm)$/i;
const AUDIO_RE = /\.(mp3|wav|ogg|oga|flac)$/i;
const PLAIN_TEXT_RE = /\.(txt|log|md|csv)$/i;

/**
 * Языки highlight.js и расширения, для которых включается подсветка.
 * SVG/HTML намеренно не входят: их содержимое показывается только как текст,
 * а inline-исполнение запрещено на уровне CSP и серверного allowlist.
 */
export const CODE_LANGUAGES: Record<string, { language: string; extensions: string[] }> = {
  json: { language: 'json', extensions: ['json'] },
  javascript: { language: 'javascript', extensions: ['js', 'mjs', 'cjs', 'jsx'] },
  typescript: { language: 'typescript', extensions: ['ts', 'mts', 'cts', 'tsx'] },
  xml: { language: 'xml', extensions: ['xml'] },
  yaml: { language: 'yaml', extensions: ['yml', 'yaml'] },
  toml: { language: 'toml', extensions: ['toml'] },
  ini: { language: 'ini', extensions: ['ini'] },
  bash: { language: 'bash', extensions: ['sh', 'bash'] },
  sql: { language: 'sql', extensions: ['sql'] },
  rust: { language: 'rust', extensions: ['rs'] },
  java: { language: 'java', extensions: ['java'] },
  properties: { language: 'properties', extensions: ['properties'] },
};

const LANGUAGE_BY_EXTENSION = new Map<string, string>(
  Object.values(CODE_LANGUAGES).flatMap(({ language, extensions }) =>
    extensions.map((extension) => [extension, language] as const),
  ),
);

export function extensionOf(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

export function isImageFile(name: string): boolean {
  return IMAGE_RE.test(name);
}

export function isVideoFile(name: string): boolean {
  return VIDEO_RE.test(name);
}

export function isAudioFile(name: string): boolean {
  return AUDIO_RE.test(name);
}

export function isPdfFile(name: string): boolean {
  return /\.pdf$/i.test(name);
}

export function isCodeFile(name: string): boolean {
  return LANGUAGE_BY_EXTENSION.has(extensionOf(name));
}

export function isPlainTextFile(name: string): boolean {
  return PLAIN_TEXT_RE.test(name);
}

/** Язык highlight.js для файла или `null`, если подсветка не нужна. */
export function languageForFile(name: string): string | null {
  return LANGUAGE_BY_EXTENSION.get(extensionOf(name)) ?? null;
}

/**
 * Выбор viewer по имени файла. Единая точка правды для иконок, доступности
 * предпросмотра и самого модального окна.
 */
export function selectViewer(name: string): PreviewKind {
  if (isPdfFile(name)) return 'pdf';
  if (isImageFile(name)) return 'image';
  if (isVideoFile(name)) return 'video';
  if (isAudioFile(name)) return 'audio';
  if (isCodeFile(name)) return 'code';
  if (isPlainTextFile(name)) return 'text';
  return 'none';
}

export function canPreviewFile(entryType: string, name: string): boolean {
  return entryType === 'file' && selectViewer(name) !== 'none';
}

export interface PreviewLimitWarning {
  title: string;
  message: string;
  limitBytes: number;
}

/**
 * Проверка размера до сетевого запроса. Возвращает понятное предупреждение
 * вместо попытки загрузить гигантский файл в память браузера.
 */
export function previewLimitWarning(
  kind: PreviewKind,
  sizeBytes: number,
): PreviewLimitWarning | null {
  if (kind === 'none' || !Number.isFinite(sizeBytes) || sizeBytes <= 0) return null;

  if (kind === 'text' || kind === 'code') {
    if (sizeBytes > PREVIEW_TEXT_MAX_BYTES) {
      return {
        title: 'Файл слишком большой для подсветки',
        message:
          'Предпросмотр текста ограничен 512 КБ. Скачайте файл, чтобы открыть его полностью.',
        limitBytes: PREVIEW_TEXT_MAX_BYTES,
      };
    }
    return null;
  }

  if (sizeBytes > PREVIEW_MAX_BYTES) {
    return {
      title: 'Файл слишком большой для предпросмотра',
      message: 'Предпросмотр ограничен 100 МБ. Скачайте файл, чтобы открыть его полностью.',
      limitBytes: PREVIEW_MAX_BYTES,
    };
  }

  return null;
}
