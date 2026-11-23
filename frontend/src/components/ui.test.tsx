import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useReducer, useState } from 'react';
import { afterEach, describe, expect, it } from 'vitest';

import { Button, Modal } from './ui';

afterEach(cleanup);

/**
 * Регрессия: ввод в поле модалки не должен уводить фокус на крестик.
 *
 * Родитель передаёт инлайн `onClose`, поэтому его identity меняется на каждом
 * keystroke. Старая реализация перезапускала effect и вызывала `.focus()` на
 * первом focusable — крестике. Здесь это воспроизводится дословно.
 */
function Harness({ revision = 0, initialOpen = true }: { revision?: number; initialOpen?: boolean }) {
  const [open, setOpen] = useState(initialOpen);
  const [value, setValue] = useState('');
  const [, rerender] = useReducer((state: number) => state + 1, 0);

  return (
    <div data-revision={revision}>
      <button type="button" onClick={() => setOpen(true)}>
        Открыть
      </button>
      <button type="button" onClick={rerender}>
        Принудительный render
      </button>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title="Форма"
        footer={
          <>
            <Button>Отмена</Button>
            <Button variant="primary">Сохранить</Button>
          </>
        }
      >
        <label htmlFor="focus-name">Название</label>
        <input
          id="focus-name"
          value={value}
          onChange={(event) => setValue(event.target.value)}
        />
      </Modal>
    </div>
  );
}

describe('Modal focus management', () => {
  it('keeps focus in the field while typing and accumulates the value', () => {
    const { rerender } = render(<Harness />);
    const field = screen.getByLabelText('Название') as HTMLInputElement;

    expect(document.activeElement).toBe(field);

    const text = 'launcher-ready';
    for (let index = 0; index < text.length; index += 1) {
      fireEvent.change(field, { target: { value: text.slice(0, index + 1) } });
      expect(document.activeElement, `фокус потерян на символе ${index + 1}`).toBe(field);
      // Внешний re-render (WS-событие, invalidateQueries) не должен ломать фокус.
      if (index % 3 === 0) rerender(<Harness revision={index} />);
    }

    expect(field.value).toBe(text);
    expect(document.activeElement).toBe(field);
  });

  it('traps Tab at both boundaries and closes on Escape returning focus to the trigger', async () => {
    render(<Harness initialOpen={false} />);
    const trigger = screen.getByRole('button', { name: 'Открыть' });
    trigger.focus();
    fireEvent.click(trigger);

    const field = screen.getByLabelText('Название') as HTMLInputElement;
    const close = screen.getByLabelText('Закрыть');
    const save = screen.getByRole('button', { name: 'Сохранить' });

    expect(document.activeElement).toBe(field);

    // Границы trap: первый focusable — крестик, последний — «Сохранить».
    close.focus();
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(save);

    fireEvent.keyDown(document, { key: 'Tab' });
    expect(document.activeElement).toBe(close);

    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

    // Фокус возвращается на элемент, который открыл модалку.
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });
});
