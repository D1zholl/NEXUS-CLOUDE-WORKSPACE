import { describe, expect, it } from 'vitest';

import {
  buildBuildTree,
  categoryForPath,
  directFiles,
  isFixedRootPath,
  nextFreePath,
  normalizeBuildPath,
  resolveDropTarget,
  validateBuildPath,
} from './buildPaths';
import type { BuildFileRow } from '../services/types';

function row(path: string, kind: 'file' | 'dir' = 'file', size = 10): BuildFileRow {
  return {
    id: path,
    buildId: 'build',
    kind,
    path,
    category: categoryForPath(path),
    side: 'client',
    required: true,
    enabled: true,
    description: '',
    objectId: null,
    sizeBytes: size,
    sha256: 'a'.repeat(64),
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
  };
}

describe('canonical build paths', () => {
  it('accepts the fixed skeleton and nested config', () => {
    expect(validateBuildPath('options.txt').ok).toBe(true);
    expect(validateBuildPath('mods/create.jar').ok).toBe(true);
    expect(validateBuildPath('shaderpacks/bsl.zip').ok).toBe(true);
    expect(validateBuildPath('resourcepacks/faithful.zip').ok).toBe(true);
    expect(validateBuildPath('config/create/deep/a.toml').ok).toBe(true);
    expect(validateBuildPath('config/create', 'dir').ok).toBe(true);
  });

  it('rejects forbidden roots, non-ASCII, reserved names and traversal', () => {
    for (const path of [
      '../escape.jar',
      '/etc/passwd',
      'C:/Windows/a.jar',
      'versions/1.20.1/client.jar',
      'assets/icons/a.png',
      'libraries/a.jar',
      '.launcher_meta.json',
      'mods/мод.jar',
      'mods/CON.jar',
      'mods/a?.jar',
      'foo/a.jar',
    ]) {
      expect(validateBuildPath(path).ok, path).toBe(false);
    }
  });

  it('enforces node rules', () => {
    expect(validateBuildPath('mods/sub/a.jar').ok).toBe(false);
    expect(validateBuildPath('mods/a.txt').ok).toBe(false);
    expect(validateBuildPath('shaderpacks/a.jar').ok).toBe(false);
    expect(validateBuildPath('resourcepacks/deep/a.zip').ok).toBe(false);
    expect(validateBuildPath('config').ok).toBe(false);
    expect(validateBuildPath('config/a.exe').ok).toBe(false);
    expect(validateBuildPath('options.txt/extra.txt').ok).toBe(false);
    expect(validateBuildPath('other-root.txt').ok).toBe(false);
  });

  it('keeps legacy categories mutable without breaking their extensions', () => {
    expect(validateBuildPath('overrides/big.bin').ok).toBe(true);
    expect(validateBuildPath('scripts/setup.zs').ok).toBe(true);
    expect(validateBuildPath('other/readme.md').ok).toBe(true);
  });

  it('normalizes separators and canonical folder case', () => {
    expect(normalizeBuildPath(' config\\Create\\..\\a.toml ')).toBe('config/Create/../a.toml');
    expect(validateBuildPath('MODS/A.JAR').normalized).toBe('mods/A.JAR');
    expect(validateBuildPath('Options.TXT').normalized).toBe('options.txt');
  });

  it('derives category from path and protects fixed nodes', () => {
    expect(categoryForPath('mods/a.jar')).toBe('mods');
    expect(categoryForPath('config/x/a.toml')).toBe('config');
    expect(categoryForPath('overrides/a.bin')).toBe('overrides');
    expect(isFixedRootPath('MODS')).toBe(true);
    expect(isFixedRootPath('config/sub')).toBe(false);
  });
});

describe('drop routing', () => {
  it('routes jars to mods, options to root and configs into config', () => {
    expect(resolveDropTarget('create.jar').targetPath).toBe('mods/create.jar');
    expect(resolveDropTarget('options.txt').targetPath).toBe('options.txt');
    expect(resolveDropTarget('settings.toml', 'create/settings.toml').targetPath).toBe(
      'config/create/settings.toml',
    );
    expect(resolveDropTarget('a.toml', 'config/deep/a.toml', 'config').targetPath).toBe(
      'config/deep/a.toml',
    );
  });

  it('keeps folder structure only where it is meaningful', () => {
    expect(resolveDropTarget('a.toml', 'MyPack/sub/a.toml', 'config').targetPath).toBe(
      'config/MyPack/sub/a.toml',
    );
    expect(resolveDropTarget('a.jar', 'MyPack/mods/a.jar', 'mods').targetPath).toBe(
      'mods/a.jar',
    );
    expect(resolveDropTarget('a.zip', 'pack/resourcepacks/a.zip').targetPath).toBe(
      'resourcepacks/a.zip',
    );
  });

  it('marks zip drops as ambiguous at the root', () => {
    const drop = resolveDropTarget('pack.zip');
    expect(drop.ambiguous).toEqual(['resourcepacks', 'shaderpacks']);
    expect(drop.targetPath).toBe('resourcepacks/pack.zip');
  });

  it('reports a concrete reason for invalid drops', () => {
    expect(resolveDropTarget('readme.exe').error).toContain('не разрешено');
    expect(resolveDropTarget('a.txt', 'a.txt', 'mods').error).toContain('.jar');
  });
});

describe('build tree', () => {
  it('always shows the fixed skeleton and aggregates counts and sizes', () => {
    const tree = buildBuildTree([
      row('options.txt', 'file', 5),
      row('mods', 'dir', 0),
      row('mods/a.jar', 'file', 100),
      row('mods/b.jar', 'file', 50),
      row('config', 'dir', 0),
      row('config/create', 'dir', 0),
      row('config/create/a.toml', 'file', 20),
      row('config/create/deep', 'dir', 0),
      row('config/create/deep/b.json', 'file', 30),
      row('overrides/legacy.bin', 'file', 7),
    ]);

    const labels = tree.children.map((node) => node.path);
    expect(labels).toContain('mods');
    expect(labels).toContain('shaderpacks');
    expect(labels).toContain('resourcepacks');
    expect(labels).toContain('config');
    expect(labels).toContain('overrides');

    const mods = tree.children.find((node) => node.path === 'mods');
    expect(mods?.totalFiles).toBe(2);
    expect(mods?.totalBytes).toBe(150);

    const config = tree.children.find((node) => node.path === 'config');
    expect(config?.totalFiles).toBe(2);
    expect(config?.totalBytes).toBe(50);

    expect(directFiles(tree, 'config/create/deep').map((file) => file.path)).toEqual([
      'config/create/deep/b.json',
    ]);
    expect(tree.totalFiles).toBe(6);
  });

  it('generates an ASCII-safe free name on conflicts', () => {
    const existing = new Set(['mods/a.jar']);
    expect(nextFreePath('mods/a.jar', existing)).toBe('mods/a (1).jar');
    expect(nextFreePath('mods/b.jar', existing)).toBe('mods/b.jar');
  });
});
