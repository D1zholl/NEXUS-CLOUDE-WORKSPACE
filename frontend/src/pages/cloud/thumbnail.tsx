import { useState } from 'react';

import { isImageFile } from '../../lib/preview';
import { api, type CloudEntry } from '../../services/api';

import { FileGlyph } from './fileIcon';

/**
 * Превью фотографии на карточке каталога.
 *
 * Пока изображение загружается (и если предпросмотр недоступен), показывается
 * обычная иконка типа файла. Браузер сам лениво подгружает картинки и
 * масштабирует их под размер карточки.
 */
export function CloudThumb({ entry }: { entry: CloudEntry }) {
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);

  if (entry.entryType !== 'file' || !isImageFile(entry.name) || failed) {
    return <FileGlyph entry={entry} />;
  }

  return (
    <>
      {!loaded ? <FileGlyph entry={entry} /> : null}
      <img
        className={`cloud-card__thumb${loaded ? ' is-loaded' : ''}`}
        src={api.cloudPreviewUrl(entry.id)}
        alt=""
        loading="lazy"
        decoding="async"
        draggable={false}
        onLoad={() => setLoaded(true)}
        onError={() => setFailed(true)}
      />
    </>
  );
}
