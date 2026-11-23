/**
 * PDF.js подключается только этим модулем, а сам модуль — динамическим
 * импортом из PdfViewer. В production worker копируется Vite в
 * `dist/assets` и загружается с того же origin.
 */
import { GlobalWorkerOptions, getDocument } from 'pdfjs-dist';
import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy } from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';

GlobalWorkerOptions.workerSrc = workerUrl;

export { getDocument, workerUrl };
export type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy };
