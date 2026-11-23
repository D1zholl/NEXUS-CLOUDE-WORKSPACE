import {
  File as FileIcon,
  FileArchive,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  Folder,
} from 'lucide-react';

import { selectViewer } from '../../lib/preview';
import type { CloudEntry } from '../../services/api';

/** Иконка файла или папки по имени и типу содержимого. */
export function FileGlyph({ entry }: { entry: CloudEntry }) {
  if (entry.entryType === 'folder') return <Folder aria-hidden="true" />;
  switch (selectViewer(entry.name)) {
    case 'image':
      return <FileImage aria-hidden="true" />;
    case 'video':
      return <FileVideo aria-hidden="true" />;
    case 'audio':
      return <FileAudio aria-hidden="true" />;
    case 'text':
    case 'code':
      return <FileText aria-hidden="true" />;
    default:
      break;
  }
  if (/\.(zip|rar|7z|tar|gz)$/i.test(entry.name)) return <FileArchive aria-hidden="true" />;
  return <FileIcon aria-hidden="true" />;
}
