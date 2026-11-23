import { Fragment, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import {
  ChevronLeft,
  ChevronRight,
  Download,
  File as FileIcon,
  Minus,
  Plus,
  TriangleAlert,
} from 'lucide-react';

import { Alert, Button, EmptyState, LoadingInline, Modal } from './ui';
import { api, type CloudEntry } from '../services/api';
import { formatBytes, formatDate } from '../lib/format';
import {
  PREVIEW_MAX_BYTES,
  PREVIEW_TEXT_MAX_BYTES,
  languageForFile,
  previewLimitWarning,
  selectViewer,
} from '../lib/preview';
import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy } from '../lib/pdf';

/* --------------------------------- предпросмотр -------------------------------- */

export function CloudPreviewModal({ entry, onClose }: { entry: CloudEntry; onClose: () => void }) {
  const kind = selectViewer(entry.name);
  const warning = previewLimitWarning(kind, entry.sizeBytes);
  const url = api.cloudPreviewUrl(entry.id);
  const language = languageForFile(entry.name);

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={entry.name}
      description={`${formatBytes(entry.sizeBytes)} · изменён ${formatDate(entry.updatedAt)}`}
      footer={
        <>
          <Button
            icon={<Download aria-hidden="true" />}
            onClick={() => triggerDownload(api.cloudDownloadUrl(entry.id))}
          >
            Скачать
          </Button>
          <Button variant="primary" onClick={onClose}>
            Закрыть
          </Button>
        </>
      }
    >
      {warning ? (
        <Alert tone="warn">
          <span className="row" style={{ gap: 8, alignItems: 'flex-start' }}>
            <TriangleAlert aria-hidden="true" style={{ width: 16, height: 16, flex: 'none' }} />
            <span>
              <strong>{warning.title}.</strong> {warning.message}
            </span>
          </span>
        </Alert>
      ) : kind === 'pdf' ? (
        <PdfViewer url={url} />
      ) : kind === 'code' || kind === 'text' ? (
        <TextPreview url={url} language={language} />
      ) : kind === 'image' || kind === 'video' || kind === 'audio' ? (
        <MediaPreview entry={entry} url={url} kind={kind} />
      ) : (
        <EmptyState icon={<FileIcon aria-hidden="true" />} title="Предпросмотр недоступен">
          Для этого типа файла предпросмотр не поддерживается — скачайте файл.
        </EmptyState>
      )}
    </Modal>
  );
}

function triggerDownload(url: string): void {
  const link = document.createElement('a');
  link.href = url;
  link.rel = 'noopener';
  link.style.display = 'none';
  document.body.appendChild(link);
  link.click();
  link.remove();
}

/* ------------------------------- текст и код ------------------------------- */

interface TextPreviewState {
  status: 'loading' | 'ready' | 'limit' | 'error';
  text?: string;
  message?: string;
}

const TEXT_LIMIT_MESSAGE =
  'Предпросмотр текста ограничен 512 КБ. Скачайте файл, чтобы открыть его полностью.';

function TextPreview({ url, language }: { url: string; language: string | null }) {
  const [state, setState] = useState<TextPreviewState>({ status: 'loading' });

  useEffect(() => {
    let active = true;
    setState({ status: 'loading' });

    fetch(url, { credentials: 'same-origin' })
      .then(async (response) => {
        if (!response.ok) throw new Error('Не удалось открыть файл.');
        const declared = Number(response.headers.get('content-length') ?? '0');
        if (declared > PREVIEW_TEXT_MAX_BYTES) {
          if (active) setState({ status: 'limit', message: TEXT_LIMIT_MESSAGE });
          return;
        }
        const content = await response.text();
        if (!active) return;
        if (new TextEncoder().encode(content).byteLength > PREVIEW_TEXT_MAX_BYTES) {
          setState({ status: 'limit', message: TEXT_LIMIT_MESSAGE });
          return;
        }
        setState({ status: 'ready', text: content });
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setState({
          status: 'error',
          message: reason instanceof Error ? reason.message : 'Ошибка предпросмотра.',
        });
      });

    return () => {
      active = false;
    };
  }, [url]);

  if (state.status === 'loading') return <LoadingInline label="Чтение файла" />;
  if (state.status === 'limit') {
    return (
      <Alert tone="warn">
        <span className="row" style={{ gap: 8, alignItems: 'flex-start' }}>
          <TriangleAlert aria-hidden="true" style={{ width: 16, height: 16, flex: 'none' }} />
          <span>{state.message}</span>
        </span>
      </Alert>
    );
  }
  if (state.status === 'error') return <Alert tone="danger">{state.message}</Alert>;

  return <HighlightedCode text={state.text ?? ''} language={language} />;
}

const HLJS_CLASS = /^hljs(?:-[a-z0-9-]+)?$/i;

/**
 * Преобразует HTML highlighter-а в React-узлы. В DOM попадают только
 * текстовые узлы и `<span>` с классами `hljs-*`, поэтому
 * `dangerouslySetInnerHTML` не используется ни для сырого файла, ни для
 * результата подсветки.
 */
function highlightedNodes(html: string): ReactNode[] {
  const parsed = new DOMParser().parseFromString(
    `<div data-highlight-root>${html}</div>`,
    'text/html',
  );
  const root = parsed.querySelector('[data-highlight-root]');
  if (!root) return [];

  const convert = (node: ChildNode, key: string): ReactNode => {
    if (node.nodeType === Node.TEXT_NODE) return node.nodeValue;

    if (node.nodeType === Node.ELEMENT_NODE) {
      const element = node as Element;
      const children = Array.from(element.childNodes).map((child, index) =>
        convert(child, `${key}-${index}`),
      );
      const classes = (element.getAttribute('class') ?? '').split(/\s+/).filter(Boolean);
      const safeSpan =
        element.tagName === 'SPAN' &&
        classes.length > 0 &&
        classes.every((className) => HLJS_CLASS.test(className));

      if (safeSpan) {
        return (
          <span key={key} className={classes.join(' ')}>
            {children.map((child, index) => (
              <Fragment key={index}>{child}</Fragment>
            ))}
          </span>
        );
      }
      return <Fragment key={key}>{children}</Fragment>;
    }

    return null;
  };

  return Array.from(root.childNodes).map((node, index) => convert(node, `hljs-${index}`));
}

function HighlightedCode({ text, language }: { text: string; language: string | null }) {
  const [highlighted, setHighlighted] = useState<ReactNode[] | null>(null);

  useEffect(() => {
    let active = true;
    setHighlighted(null);
    if (!language) return;

    import('../lib/highlight')
      .then(({ highlightCode }) => {
        if (!active) return;
        const result = highlightCode(text, language);
        if (result) setHighlighted(highlightedNodes(result.html));
      })
      .catch(() => {
        // Подсветка не критична: остаётся обычный текст.
      });

    return () => {
      active = false;
    };
  }, [text, language]);

  return (
    <pre className="cloud-preview-text cloud-preview-code" data-language={language ?? undefined}>
      {highlighted ?? text}
    </pre>
  );
}

/* --------------------------------- медиа ---------------------------------- */

function MediaPreview({
  entry,
  url,
  kind,
}: {
  entry: CloudEntry;
  url: string;
  kind: 'image' | 'video' | 'audio';
}) {
  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let objectUrl: string | null = null;
    setBlobUrl(null);
    setError(null);

    fetch(url, { credentials: 'same-origin' })
      .then(async (response) => {
        if (!response.ok) throw new Error('Не удалось открыть файл.');
        const blob = await response.blob();
        objectUrl = URL.createObjectURL(blob);
        if (active) setBlobUrl(objectUrl);
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setError(reason instanceof Error ? reason.message : 'Ошибка предпросмотра.');
      });

    return () => {
      active = false;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [url]);

  if (error) return <Alert tone="danger">{error}</Alert>;

  if (kind === 'image') {
    return blobUrl ? (
      <img className="cloud-preview-media" src={blobUrl} alt={entry.name} />
    ) : (
      <LoadingInline label="Загрузка изображения" />
    );
  }

  if (kind === 'video') {
    return blobUrl ? (
      <video className="cloud-preview-media" src={blobUrl} controls />
    ) : (
      <LoadingInline label="Загрузка видео" />
    );
  }

  return blobUrl ? (
    <audio className="cloud-preview-audio" src={blobUrl} controls />
  ) : (
    <LoadingInline label="Загрузка аудио" />
  );
}

/* ---------------------------------- PDF ----------------------------------- */

function PdfViewer({ url }: { url: string }) {
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pageNumber, setPageNumber] = useState(1);
  const [scale, setScale] = useState(1);
  const [rendering, setRendering] = useState(false);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    let active = true;
    let loadingTask: PDFDocumentLoadingTask | null = null;
    setDocument(null);
    setError(null);
    setPageNumber(1);
    setScale(1);

    (async () => {
      const response = await fetch(url, { credentials: 'same-origin' });
      if (!response.ok) throw new Error('Не удалось открыть PDF.');
      const declared = Number(response.headers.get('content-length') ?? '0');
      if (declared > PREVIEW_MAX_BYTES) {
        throw new Error('Предпросмотр PDF ограничен 100 МБ. Скачайте файл.');
      }

      const data = new Uint8Array(await response.arrayBuffer());
      if (data.byteLength > PREVIEW_MAX_BYTES) {
        throw new Error('Предпросмотр PDF ограничен 100 МБ. Скачайте файл.');
      }

      const pdfjs = await import('../lib/pdf');
      loadingTask = pdfjs.getDocument({
        data,
        disableAutoFetch: true,
      });
      const pdf = await loadingTask.promise;
      if (!active) {
        void loadingTask.destroy();
        return;
      }
      setDocument(pdf);
    })().catch((reason: unknown) => {
      if (!active) return;
      setError(reason instanceof Error ? reason.message : 'Ошибка предпросмотра PDF.');
    });

    return () => {
      active = false;
      if (loadingTask) void loadingTask.destroy();
    };
  }, [url]);

  useEffect(() => {
    if (!document) return;
    let active = true;
    let renderTask: { cancel: () => void; promise: Promise<void> } | null = null;
    setRendering(true);

    (async () => {
      const page: PDFPageProxy = await document.getPage(pageNumber);
      if (!active) return;
      const canvas = canvasRef.current;
      if (!canvas) return;

      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      const viewport = page.getViewport({ scale });
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;

      const context = canvas.getContext('2d');
      if (!context) throw new Error('Canvas недоступен.');
      context.setTransform(ratio, 0, 0, ratio, 0, 0);

      renderTask = page.render({ canvas: null, canvasContext: context, viewport });
      await renderTask.promise;
      if (active) setRendering(false);
    })().catch((reason: unknown) => {
      const name = reason instanceof Error ? reason.name : '';
      if (name === 'RenderingCancelledException' || name === 'AbortException') return;
      if (!active) return;
      setRendering(false);
      setError(reason instanceof Error ? reason.message : 'Не удалось отрисовать страницу PDF.');
    });

    return () => {
      active = false;
      renderTask?.cancel();
    };
  }, [document, pageNumber, scale]);

  if (error) return <Alert tone="danger">{error}</Alert>;
  if (!document) return <LoadingInline label="Загрузка PDF" />;

  const total = document.numPages;

  return (
    <div className="cloud-preview-pdf">
      <div className="cloud-preview-pdf__toolbar">
        <Button
          size="sm"
          iconOnly
          aria-label="Предыдущая страница"
          disabled={pageNumber <= 1}
          onClick={() => setPageNumber((value) => Math.max(1, value - 1))}
        >
          <ChevronLeft aria-hidden="true" />
        </Button>
        <span className="tiny muted cloud-preview-pdf__counter" aria-live="polite">
          Страница {pageNumber} из {total}
        </span>
        <Button
          size="sm"
          iconOnly
          aria-label="Следующая страница"
          disabled={pageNumber >= total}
          onClick={() => setPageNumber((value) => Math.min(total, value + 1))}
        >
          <ChevronRight aria-hidden="true" />
        </Button>

        <span className="cloud-preview-pdf__divider" aria-hidden="true" />

        <Button
          size="sm"
          iconOnly
          aria-label="Уменьшить масштаб"
          disabled={scale <= 0.5}
          onClick={() => setScale((value) => Math.max(0.5, Math.round((value - 0.25) * 100) / 100))}
        >
          <Minus aria-hidden="true" />
        </Button>
        <span className="tiny muted cloud-preview-pdf__zoom">{Math.round(scale * 100)}%</span>
        <Button
          size="sm"
          iconOnly
          aria-label="Увеличить масштаб"
          disabled={scale >= 3}
          onClick={() => setScale((value) => Math.min(3, Math.round((value + 0.25) * 100) / 100))}
        >
          <Plus aria-hidden="true" />
        </Button>
      </div>

      <div className="cloud-preview-pdf__stage" aria-busy={rendering}>
        <canvas ref={canvasRef} aria-label={`Страница ${pageNumber} из ${total}`} />
      </div>
    </div>
  );
}
