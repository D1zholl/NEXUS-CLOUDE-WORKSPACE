/** Верхняя панель Explorer: создание, загрузка, обновление, поиск. */
import {
  ChevronDown,
  FilePlus2,
  FolderPlus,
  ListFilter,
  RefreshCw,
  Search,
  Upload,
  X,
} from 'lucide-react';

import type { BottomTab } from '../../../stores/ai/agentWorkspace';

export type ExplorerSearchMode = 'name' | 'content';
export type ExplorerSort = 'name' | 'type' | 'modified';

interface WorkspaceToolbarProps {
  canWrite: boolean;
  searchOpen: boolean;
  searchMode: ExplorerSearchMode;
  query: string;
  busy: boolean;
  onToggleSearch: () => void;
  onSearchMode: (mode: ExplorerSearchMode) => void;
  onQuery: (value: string) => void;
  onSearch: () => void;
  sort: ExplorerSort;
  onSort: (sort: ExplorerSort) => void;
  onCreateFile: () => void;
  onCreateFolder: () => void;
  onUpload: () => void;
  onRefresh: () => void;
  onCollapseAll: () => void;
  bottomTab: BottomTab;
  onBottomTab: (tab: BottomTab) => void;
}

export function WorkspaceToolbar({
  canWrite,
  searchOpen,
  searchMode,
  query,
  busy,
  onToggleSearch,
  onSearchMode,
  onQuery,
  onSearch,
  sort,
  onSort,
  onCreateFile,
  onCreateFolder,
  onUpload,
  onRefresh,
  onCollapseAll,
  bottomTab,
  onBottomTab,
}: WorkspaceToolbarProps) {
  return (
    <div className="ws-toolbar">
      <div className="ws-toolbar__row">
        <span className="ws-toolbar__title">Explorer</span>
        <div className="ws-toolbar__actions">
          <button
            type="button"
            className="ai-icon-btn"
            title="Новый файл"
            onClick={onCreateFile}
            disabled={!canWrite}
          >
            <FilePlus2 aria-hidden="true" />
          </button>
          <button
            type="button"
            className="ai-icon-btn"
            title="Новая папка"
            onClick={onCreateFolder}
            disabled={!canWrite}
          >
            <FolderPlus aria-hidden="true" />
          </button>
          <button
            type="button"
            className="ai-icon-btn"
            title="Загрузить файлы"
            onClick={onUpload}
            disabled={!canWrite}
          >
            <Upload aria-hidden="true" />
          </button>
          <button
            type="button"
            className="ai-icon-btn"
            title="Обновить дерево"
            onClick={onRefresh}
            disabled={busy}
          >
            <RefreshCw aria-hidden="true" className={busy ? 'spin' : undefined} />
          </button>
          <button
            type="button"
            className={`ai-icon-btn${searchOpen ? ' is-active' : ''}`}
            title="Поиск"
            onClick={onToggleSearch}
          >
            <Search aria-hidden="true" />
          </button>
          <button
            type="button"
            className="ai-icon-btn"
            title="Свернуть всё"
            onClick={onCollapseAll}
          >
            <ChevronDown aria-hidden="true" />
          </button>
        </div>
      </div>
      {searchOpen ? (
        <div className="ws-search">
          <div className="ws-search__modes">
            <button
              type="button"
              className={searchMode === 'name' ? 'is-active' : ''}
              onClick={() => onSearchMode('name')}
            >
              Имена
            </button>
            <button
              type="button"
              className={searchMode === 'content' ? 'is-active' : ''}
              onClick={() => onSearchMode('content')}
            >
              <ListFilter aria-hidden="true" /> Содержимое
            </button>
          </div>
          <form
            className="ws-search__form"
            onSubmit={(event) => {
              event.preventDefault();
              onSearch();
            }}
          >
            <input
              autoFocus
              value={query}
              placeholder={searchMode === 'name' ? 'Имя файла или папки' : 'Текст в файлах'}
              onChange={(event) => onQuery(event.target.value)}
            />
            <button type="submit" className="ai-icon-btn" title="Найти">
              <Search aria-hidden="true" />
            </button>
            <button
              type="button"
              className="ai-icon-btn"
              title="Закрыть поиск"
              onClick={onToggleSearch}
            >
              <X aria-hidden="true" />
            </button>
          </form>
        </div>
      ) : null}
      <div className="ws-toolbar__tabs">
        <select
          className="ws-toolbar__sort"
          value={sort}
          onChange={(event) => onSort(event.target.value as ExplorerSort)}
          aria-label="Сортировка файлов"
          title="Сортировка"
        >
          <option value="name">По имени</option>
          <option value="type">По типу</option>
          <option value="modified">По изменению</option>
        </select>
        <button
          type="button"
          className={bottomTab === 'changes' ? 'is-active' : ''}
          onClick={() => onBottomTab('changes')}
          title="Изменённые файлы"
        >
          Changes
        </button>
        <button
          type="button"
          className={bottomTab === 'usage' ? 'is-active' : ''}
          onClick={() => onBottomTab('usage')}
          title="Использование workspace"
        >
          Usage
        </button>
      </div>
    </div>
  );
}
