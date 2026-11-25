/** Редактор кода NEXUS: подсветка, нумерация строк, поиск, Ctrl+S. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronUp, Search, X } from 'lucide-react';

import { useHighlightedCode } from './HighlightedCode';

const LINE_HEIGHT = 20;

interface CodeEditorProps {
  path: string;
  content: string;
  language: string | null;
  readOnly?: boolean;
  onChange: (value: string) => void;
  onSave?: () => void;
}

export function CodeEditor({
  path,
  content,
  language,
  readOnly = false,
  onChange,
  onSave,
}: CodeEditorProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const highlightRef = useRef<HTMLPreElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [matchIndex, setMatchIndex] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(600);
  const highlighted = useHighlightedCode(content, language);

  // Смена файла: сбрасываем поиск и позицию скролла.
  useEffect(() => {
    setQuery('');
    setSearchOpen(false);
    setMatchIndex(0);
    setScrollTop(0);
    if (textareaRef.current) textareaRef.current.scrollTop = 0;
  }, [path]);

  const totalLines = useMemo(() => {
    if (content.length === 0) return 1;
    let count = 1;
    let index = content.indexOf('\n');
    while (index !== -1) {
      count += 1;
      index = content.indexOf('\n', index + 1);
    }
    return count;
  }, [content]);
  const firstVisible = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - 2);
  const visibleCount = Math.ceil(viewportHeight / LINE_HEIGHT) + 5;
  const lastVisible = Math.min(totalLines, firstVisible + visibleCount);
  const matches = (() => {
    if (!query) return [] as number[];
    const needle = query.toLowerCase();
    const haystack = content.toLowerCase();
    const found: number[] = [];
    let index = haystack.indexOf(needle);
    while (index !== -1 && found.length < 2000) {
      found.push(index);
      index = haystack.indexOf(needle, index + Math.max(1, needle.length));
    }
    return found;
  })();

  useEffect(() => {
    setMatchIndex(0);
  }, [query]);

  const scrollToOffset = useCallback((offset: number) => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const before = textarea.value.slice(0, offset);
    const line = before.split('\n').length - 1;
    const target = Math.max(0, line * LINE_HEIGHT - viewportHeight / 2);
    textarea.scrollTop = target;
    if (highlightRef.current) highlightRef.current.scrollTop = target;
    setScrollTop(target);
  }, [viewportHeight]);

  const jumpToMatch = useCallback(
    (direction: 1 | -1) => {
      if (matches.length === 0) return;
      const next = (matchIndex + direction + matches.length) % matches.length;
      setMatchIndex(next);
      const offset = matches[next];
      const textarea = textareaRef.current;
      if (textarea) {
        textarea.focus();
        textarea.setSelectionRange(offset, offset + query.length);
      }
      scrollToOffset(offset);
    },
    [matchIndex, matches, query.length, scrollToOffset],
  );

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const meta = event.ctrlKey || event.metaKey;
    if (meta && event.key.toLowerCase() === 's') {
      event.preventDefault();
      onSave?.();
      return;
    }
    if (meta && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      setSearchOpen(true);
      return;
    }
    if (event.key === 'Escape' && searchOpen) {
      setSearchOpen(false);
      return;
    }
    if (event.key === 'Tab' && !readOnly) {
      event.preventDefault();
      const textarea = event.currentTarget;
      const start = textarea.selectionStart;
      const end = textarea.selectionEnd;
      const next = `${content.slice(0, start)}  ${content.slice(end)}`;
      onChange(next);
      window.requestAnimationFrame(() => {
        textarea.selectionStart = start + 2;
        textarea.selectionEnd = start + 2;
      });
    }
  };

  const handleScroll = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    if (highlightRef.current) {
      highlightRef.current.scrollTop = textarea.scrollTop;
      highlightRef.current.scrollLeft = textarea.scrollLeft;
    }
    setScrollTop(textarea.scrollTop);
  };

  useEffect(() => {
    const element = textareaRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setViewportHeight(element.clientHeight));
    observer.observe(element);
    setViewportHeight(element.clientHeight);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="code-editor">
      {searchOpen ? (
        <div className="code-editor__search">
          <Search aria-hidden="true" />
          <input
            autoFocus
            value={query}
            placeholder="Найти в файле"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                jumpToMatch(event.shiftKey ? -1 : 1);
              }
              if (event.key === 'Escape') setSearchOpen(false);
            }}
          />
          <span className="code-editor__search-count">
            {query ? `${matches.length === 0 ? 0 : matchIndex + 1}/${matches.length}` : ''}
          </span>
          <button type="button" className="ai-icon-btn" onClick={() => jumpToMatch(-1)} title="Предыдущее">
            <ChevronUp aria-hidden="true" />
          </button>
          <button type="button" className="ai-icon-btn" onClick={() => jumpToMatch(1)} title="Следующее">
            <ChevronDown aria-hidden="true" />
          </button>
          <button type="button" className="ai-icon-btn" onClick={() => setSearchOpen(false)} title="Закрыть">
            <X aria-hidden="true" />
          </button>
        </div>
      ) : null}
      <div className="code-editor__body">
        <div className="code-editor__gutter" ref={gutterRef} aria-hidden="true">
          <div style={{ paddingTop: firstVisible * LINE_HEIGHT }}>
            {Array.from({ length: Math.max(0, lastVisible - firstVisible) }, (_, index) => (
              <div key={firstVisible + index} className="code-editor__ln">
                {firstVisible + index + 1}
              </div>
            ))}
          </div>
        </div>
        <div className="code-editor__area">
          <pre className="code-editor__highlight" ref={highlightRef} aria-hidden="true">
            <code>{highlighted ?? content}</code>
            {'\n'}
          </pre>
          <textarea
            ref={textareaRef}
            className="code-editor__input"
            value={content}
            readOnly={readOnly}
            spellCheck={false}
            wrap="off"
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={handleKeyDown}
            onScroll={handleScroll}
            aria-label={`Редактор ${path}`}
          />
        </div>
      </div>
      <div className="code-editor__status">
        <span>{totalLines} строк</span>
        <span>{language ?? 'plain text'}</span>
        {readOnly ? <span>только чтение</span> : null}
      </div>
    </div>
  );
}
