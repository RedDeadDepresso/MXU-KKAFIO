/**
 * Game settings resolution (GamePath + GameType).
 *
 * The CLI falls back to the first other instance that has a GamePath when an
 * instance has none, and copies GamePath and GameType together from it.
 * The GUI mirrors that rule here so the panel can show what will actually be
 * used. Inherited values are never written into the instance itself.
 */

import type { Instance } from '@/types/interface';

export const GAME_TYPES = [
  { value: 'Koikatsu', label: 'Koikatsu', exe: ['Koikatu.exe'] },
  { value: 'KoikatsuParty', label: 'Koikatsu Party', exe: ['Koikatsu Party.exe'] },
  {
    value: 'KoikatsuSunshine',
    label: 'Koikatsu Sunshine',
    exe: ['KoikatsuSunshine.exe', 'Koikatsu Sunshine.exe'],
  },
] as const;

export type GameTypeValue = (typeof GAME_TYPES)[number]['value'];

export const DEFAULT_GAME_TYPE: GameTypeValue = 'Koikatsu';

export interface GameSettingsSource {
  id: string;
  name: string;
  path: string;
  type: GameTypeValue;
}

export interface ResolvedGameSettings {
  /** Effective game path ('' if none is available anywhere) */
  path: string;
  /** Effective game type */
  type: GameTypeValue;
  /** True when the values come from another instance */
  inherited: boolean;
  /** The instance's own stored values (path is '' when not set) */
  own: { path: string; type?: GameTypeValue };
  /** First other instance that has a game path, if any */
  source?: GameSettingsSource;
}

function readOwn(instance: Instance | undefined): { path: string; type?: GameTypeValue } {
  const gv = instance?.globalOptionValues ?? {};
  const pathValue = gv['GamePath'];
  const typeValue = gv['GameType'];
  return {
    path: pathValue?.type === 'folder' ? pathValue.path.trim() : '',
    type: typeValue?.type === 'select' ? (typeValue.caseName as GameTypeValue) : undefined,
  };
}

export function resolveGameSettings(
  instances: Instance[],
  instanceId: string,
): ResolvedGameSettings {
  const own = readOwn(instances.find((i) => i.id === instanceId));

  let source: GameSettingsSource | undefined;
  for (const inst of instances) {
    if (inst.id === instanceId) continue;
    const o = readOwn(inst);
    if (o.path) {
      source = { id: inst.id, name: inst.name, path: o.path, type: o.type ?? DEFAULT_GAME_TYPE };
      break;
    }
  }

  if (own.path) {
    return { path: own.path, type: own.type ?? DEFAULT_GAME_TYPE, inherited: false, own, source };
  }
  if (source) {
    return { path: source.path, type: source.type, inherited: true, own, source };
  }
  return {
    path: '',
    type: own.type ?? DEFAULT_GAME_TYPE,
    inherited: false,
    own,
    source: undefined,
  };
}
