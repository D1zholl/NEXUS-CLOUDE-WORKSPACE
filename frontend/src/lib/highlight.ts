/**
 * Ленивый highlight.js: core + только нужные языки.
 *
 * Модуль подключается через `import('../lib/highlight')` в момент открытия
 * текстового файла, поэтому он попадает в отдельный chunk и не утяжеляет
 * основной бандл.
 */
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import properties from 'highlight.js/lib/languages/properties';
import rust from 'highlight.js/lib/languages/rust';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';

for (const [name, language] of Object.entries({
  bash,
  ini,
  java,
  javascript,
  json,
  properties,
  rust,
  sql,
  typescript,
  xml,
  yaml,
})) {
  hljs.registerLanguage(name, language);
}

// Отдельного грамматического модуля TOML в highlight.js 11 нет:
// используем INI-грамматику и регистрируем `toml` как алиас.
hljs.registerAliases(['toml'], { languageName: 'ini' });

export interface HighlightResult {
  html: string;
  language: string;
}

/**
 * Возвращает HTML только из проверенного highlighter-а. Этот HTML нельзя
 * вставлять через `dangerouslySetInnerHTML`: компонент разбирает его в
 * React-узлы, сохраняя только `<span class="hljs-*">` и текстовые узлы.
 */
export function highlightCode(code: string, language: string): HighlightResult | null {
  if (!hljs.getLanguage(language)) return null;
  try {
    const result = hljs.highlight(code, { language, ignoreIllegals: true });
    return { html: result.value, language: result.language ?? language };
  } catch {
    return null;
  }
}
