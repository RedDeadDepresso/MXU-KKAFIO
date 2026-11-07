/**
 * GlobalOptionPanel — Instance Settings
 *
 * Pinned, collapsible card at the top of the task list showing GamePath and
 * GameType. Values are stored per-instance in `globalOptionValues` so each
 * configuration slot can target a different game installation.
 *
 * When an instance has no GamePath, the CLI copies GamePath + GameType from the
 * first other instance that has one. The panel shows that inherited pair
 * read-only (see utils/gameSettings.ts); "Customize" copies it into this
 * instance, and "Reset" clears both values again.
 *
 * Not draggable, has no checkbox, always rendered first.
 */

import { useState, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { Settings2, ChevronDown, ChevronRight, Link2 } from 'lucide-react';
import { toast } from 'sonner';
import clsx from 'clsx';
import { useAppStore } from '@/stores/appStore';
import { GAME_TYPES, resolveGameSettings, type GameTypeValue } from '@/utils/gameSettings';

// ── Dropdown menu ─────────────────────────────────────────────────────────────

interface MenuItem { label: string; disabled?: boolean; danger?: boolean; onClick: () => void; }

function DropdownMenu({ items }: { items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const h = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false); };
    document.addEventListener('mousedown', h);
    return () => document.removeEventListener('mousedown', h);
  }, [open]);
  return (
    <div className="relative flex-shrink-0" ref={ref}>
      <button type="button" onClick={() => setOpen(o => !o)}
        className="flex items-center px-2.5 py-1.5 rounded-md bg-bg-tertiary border border-border text-sm text-text-secondary hover:bg-bg-hover hover:text-text-primary transition-colors select-none"
        aria-haspopup="menu" aria-expanded={open}>⋯</button>
      {open && (
        <div className="absolute right-0 mt-1 z-50 min-w-[170px] py-1 rounded-lg border border-border bg-bg-secondary shadow-lg">
          {items.map(item => (
            <button key={item.label} type="button" disabled={item.disabled}
              onClick={() => { setOpen(false); item.onClick(); }}
              className={clsx('w-full text-left px-3 py-1.5 text-sm transition-colors',
                item.disabled ? 'text-text-muted cursor-not-allowed'
                  : item.danger ? 'text-error hover:bg-error/10'
                  : 'text-text-primary hover:bg-bg-hover')}>
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Main component ─────────────────────────────────────────────────────────────

interface GlobalOptionPanelProps { instanceId: string; }

export function GlobalOptionPanel({ instanceId }: GlobalOptionPanelProps) {
  const { t } = useTranslation();
  // Path text being edited; committed on blur so a half-typed (or emptied) path
  // doesn't flip the card into the inherited state mid-edit.
  const [draft, setDraft] = useState<string | null>(null);
  const {
    instances,
    setGlobalOptionValue,
    clearGlobalOptionValues,
    gameSettingsExpanded,
    setGameSettingsExpanded,
  } = useAppStore();
  const collapsed = !gameSettingsExpanded; // persisted across restarts

  const instance = instances.find(i => i.id === instanceId);
  if (!instance) return null;

  const resolved = resolveGameSettings(instances, instanceId);
  const { inherited, source } = resolved;
  const folderPath = resolved.path; // effective path (own or inherited)
  const gameType: GameTypeValue = resolved.type; // effective type
  const gameTypeLabel = GAME_TYPES.find((gt) => gt.value === gameType)?.label ?? gameType;

  const setPath = (p: string) =>
    setGlobalOptionValue(instanceId, 'GamePath', { type: 'folder', path: p });
  const setType = (v: GameTypeValue) =>
    setGlobalOptionValue(instanceId, 'GameType', { type: 'select', caseName: v });

  /** Store a path as this instance's own; copy the inherited type along when leaving inheritance. */
  const setOwnPath = (p: string) => {
    if (inherited) setType(resolved.type);
    setPath(p);
  };

  const customize = () => {
    setPath(resolved.path);
    setType(resolved.type);
  };

  /** Drop this instance's own values so it inherits again (type only if there is something to inherit). */
  const resetToInherited = () =>
    clearGlobalOptionValues(instanceId, source ? ['GamePath', 'GameType'] : ['GamePath']);

  const commitDraft = () => {
    if (draft === null) return;
    const p = draft.trim();
    setDraft(null);
    if (p === resolved.own.path) return;
    if (p) setPath(p);
    else resetToInherited();
  };

  const handleBrowse = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === 'string' && selected) {
        setDraft(null);
        setOwnPath(selected);
      }
    } catch { /* cancelled */ }
  };

  const handleShowInExplorer = async () => {
    if (!folderPath) return;
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('open_file', { filePath: folderPath });
    } catch { /* ignore */ }
  };

  const handleRunGame = async () => {
    if (!folderPath) return;
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const result = await invoke<{ ok: boolean; exe: string; error: string }>(
        'kkafio_run_game', { gamePath: folderPath, gameType },
      );
      if (result.ok) {
        const exeName = result.exe.replace(/\\/g, '/').split('/').pop() ?? result.exe;
        toast.success(`Launched ${exeName}`);
      } else {
        toast.error(result.error);
      }
    } catch (e) {
      toast.error(`Failed to launch game: ${e}`);
    }
  };

  const handleRunStudio = async () => {
    if (!folderPath) return;
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const result = await invoke<{ ok: boolean; exe: string; error: string }>(
        'kkafio_run_studio', { gamePath: folderPath },
      );
      if (result.ok) {
        const exeName = result.exe.replace(/\\/g, '/').split('/').pop() ?? result.exe;
        toast.success(`Launched ${exeName}`);
      } else {
        toast.error(result.error);
      }
    } catch (e) {
      toast.error(`Failed to launch studio: ${e}`);
    }
  };

  const menuItems: MenuItem[] = [
    { label: t('options.folder.showInExplorer', 'Show in Explorer'), disabled: !folderPath, onClick: handleShowInExplorer },
    { label: t('options.folder.browse', 'Browse…'), onClick: handleBrowse },
    { label: t('options.gameFolder.run', 'Run Game'), disabled: !folderPath, onClick: handleRunGame },
    { label: t('options.gameFolder.runStudio', 'Run Studio'), disabled: !folderPath, onClick: handleRunStudio },
  ];

  return (
    <div className="rounded-lg border border-accent/30 bg-bg-secondary shadow-sm overflow-visible">
      {/* Header */}
      <button type="button" onClick={() => setGameSettingsExpanded(!gameSettingsExpanded)}
        className="w-full flex items-center gap-2 px-3 py-2 bg-accent/5 border-b border-accent/20 hover:bg-accent/10 transition-colors rounded-t-lg"
        aria-expanded={!collapsed}>
        <Settings2 className="w-3.5 h-3.5 text-accent flex-shrink-0" />
        <span className="text-left text-xs font-semibold text-accent tracking-wide uppercase">
          {t('instanceSettings.title', 'Game Settings')}
        </span>
        <span
          className="flex-1 min-w-0 truncate text-left text-[11px] text-text-muted"
          title={collapsed && folderPath ? folderPath : undefined}
        >
          {collapsed && folderPath
            ? `${gameTypeLabel} · ${folderPath}${inherited ? ` (${t('instanceSettings.inherited', 'inherited')})` : ''}`
            : ''}
        </span>
        {collapsed ? <ChevronRight className="w-3.5 h-3.5 text-accent/60" /> : <ChevronDown className="w-3.5 h-3.5 text-accent/60" />}
      </button>

      {!collapsed && (
        <div className="p-3 space-y-3">

          {/* Inherited-from banner */}
          {inherited && source && (
            <div className="flex items-center gap-2 rounded-md border border-accent/20 bg-accent/10 px-2.5 py-1.5">
              <Link2 className="w-3.5 h-3.5 text-accent flex-shrink-0" />
              <span className="flex-1 min-w-0 truncate text-xs text-text-secondary" title={source.name}>
                {t('instanceSettings.inheritedFrom', {
                  name: source.name,
                  defaultValue: 'Using Game Settings from "{{name}}"',
                })}
              </span>
              <button type="button" onClick={customize}
                className="flex-shrink-0 px-2 py-0.5 rounded-md text-xs text-accent border border-accent/40 hover:bg-accent/10 transition-colors">
                {t('instanceSettings.customize', 'Customize')}
              </button>
            </div>
          )}

          {/* GameType select */}
          <div className="space-y-1">
            <p className="text-sm font-medium text-text-primary">
              {t('instanceSettings.gameType', 'Game Type')}
            </p>
            <div className="flex gap-1 flex-wrap">
              {GAME_TYPES.map(gt => (
                <button key={gt.value} type="button"
                  disabled={inherited}
                  onClick={() => setType(gt.value)}
                  className={clsx(
                    'px-3 py-1.5 rounded-md text-sm transition-colors border',
                    gameType === gt.value
                      ? 'bg-accent text-white border-accent'
                      : 'bg-bg-primary text-text-secondary border-border',
                    inherited
                      ? 'opacity-60 cursor-not-allowed'
                      : gameType !== gt.value && 'hover:bg-bg-hover hover:text-text-primary',
                  )}>
                  {gt.label}
                </button>
              ))}
            </div>
          </div>

          {/* GamePath folder */}
          <div className="space-y-1">
            <p className="text-sm font-medium text-text-primary">
              {t('instanceSettings.gamePath', 'Game Installation Path')}
            </p>
            <div className="flex gap-2 items-center">
              <input type="text"
                value={inherited ? folderPath : (draft ?? resolved.own.path)}
                readOnly={inherited}
                onChange={e => setDraft(e.target.value)}
                onBlur={commitDraft}
                onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); }}
                placeholder={t('options.folder.placeholder', 'Select a folder…')}
                className={clsx(
                  'flex-1 min-w-0 px-3 py-1.5 rounded-md bg-bg-primary border border-border text-sm placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-accent font-mono',
                  inherited ? 'text-text-muted italic cursor-default' : 'text-text-primary',
                )} />
              <DropdownMenu items={menuItems} />
            </div>
            {!inherited && source && resolved.own.path && (
              <button type="button" onClick={resetToInherited}
                className="text-xs text-accent hover:underline">
                {t('instanceSettings.resetToInherited', {
                  name: source.name,
                  defaultValue: 'Use settings from "{{name}}"',
                })}
              </button>
            )}
            {!folderPath && !source && !draft?.trim() && (
              <p className="text-xs text-warning">
                {t('instanceSettings.noPath', 'No game path set. Set one here or in another instance.')}
              </p>
            )}
          </div>

        </div>
      )}
    </div>
  );
}
