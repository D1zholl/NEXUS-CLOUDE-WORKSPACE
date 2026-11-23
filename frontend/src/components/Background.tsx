import { useEffect, useRef, useState } from 'react';
import { useLocation } from 'react-router-dom';

import type { BackgroundSettings } from '../services/api';
import { useBackground } from '../stores/background';
import { BackgroundEngine } from '../three/engine';

interface BackgroundCanvasProps {
  settings: BackgroundSettings;
  preview?: boolean;
}

/**
 * Один экземпляр WebGL-движка на канвас.
 * Движок создаётся один раз и обновляет параметры без пересоздания сцены.
 */
export function BackgroundCanvas({ settings, preview = false }: BackgroundCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<BackgroundEngine | null>(null);
  const settingsRef = useRef(settings);
  const [failed, setFailed] = useState(false);
  settingsRef.current = settings;

  useEffect(() => {
    if (!settings.enabled) {
      setFailed(false);
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas) return;

    let engine: BackgroundEngine | null = null;
    const initTimer = window.setTimeout(() => {
      try {
        engine = new BackgroundEngine(canvas, settingsRef.current, { preview });
        engineRef.current = engine;
        setFailed(false);
      } catch (error) {
        // WebGL может быть недоступен: показываем статичный градиент вместо пустоты.
        console.warn('3D-фон недоступен, используется статичная подложка.', error);
        setFailed(true);
      }
    }, 100);

    return () => {
      window.clearTimeout(initTimer);
      engine?.dispose();
      engineRef.current = null;
    };
    // Настройки передаются через setSettings; пересоздание нужно только при
    // смене режима и включении/выключении фона.
  }, [preview, settings.enabled]);

  useEffect(() => {
    engineRef.current?.setSettings(settings);
  }, [settings]);

  if (!settings.enabled || failed) {
    return <div className="bg-fallback" aria-hidden="true" />;
  }

  return <canvas ref={canvasRef} aria-hidden="true" />;
}

/**
 * Глобальный фон приложения. На странице настроек 3D-фона скрывается,
 * чтобы не занимать второй WebGL-контекст рядом с предпросмотром.
 */
export function GlobalBackground() {
  const settings = useBackground((state) => state.settings);
  const location = useLocation();

  if (location.pathname === '/system/background') return null;

  return (
    <div className="bg-layer" aria-hidden="true">
      <BackgroundCanvas settings={settings} />
      <div className="bg-layer__vignette" />
    </div>
  );
}