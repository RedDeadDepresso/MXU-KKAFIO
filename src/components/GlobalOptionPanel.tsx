/**
 * GlobalOptionPanel — Instance Settings
 *
 * Pinned, collapsible card at the top of the task list showing GamePath and
 * GameType. Values are stored per-instance in `globalOptionValues` so each
 * configuration slot can target a different game installation.
 *
 * Not draggable, has no checkbox, always rendered first.
 */

import { useState, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { Settings2, ChevronDown, ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import clsx from 'clsx';
import { useAppStore } from '@/stores/appStore';

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

// ── Game types ────────────────────────────────────────────────────────────────

const GAME_TYPES = [
  { value: 'Koikatsu',        label: 'Koikatsu',         exe: ['Koikatsu.exe'] },
  { value: 'KoikatsuParty',   label: 'Koikatsu Party',   exe: ['Koikatsu Party.exe'] },
  { value: 'KoikatsuSunshine',label: 'Koikatsu Sunshine',exe: ['KoikatsuSunshine.exe', 'Koikatsu Sunshine.exe'] },
] as const;

type GameTypeValue = typeof GAME_TYPES[number]['value'];

// ── Main component ─────────────────────────────────────────────────────────────

interface GlobalOptionPanelProps { instanceId: string; }

export function GlobalOptionPanel({ instanceId }: GlobalOptionPanelProps) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(false);
  const { instances, setGlobalOptionValue } = useAppStore();

  const instance = instances.find(i => i.id === instanceId);
  if (!instance) return null;

  const gv = instance.globalOptionValues ?? {};

  const folderPath: string  = gv['GamePath']?.type === 'folder'  ? gv['GamePath'].path  : '';
  const gameType: GameTypeValue =
    (gv['GameType']?.type === 'select' ? gv['GameType'].caseName : 'Koikatsu') as GameTypeValue;

  const setPath = (p: string) =>
    setGlobalOptionValue(instanceId, 'GamePath', { type: 'folder', path: p });
  const setType = (v: GameTypeValue) =>
    setGlobalOptionValue(instanceId, 'GameType', { type: 'select', caseName: v });

  const handleBrowse = async () => {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === 'string' && selected) setPath(selected);
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

  const menuItems: MenuItem[] = [
    { label: t('options.folder.showInExplorer', 'Show in Explorer'), disabled: !folderPath, onClick: handleShowInExplorer },
    { label: t('options.folder.browse', 'Browse…'), onClick: handleBrowse },
    { label: t('options.gameFolder.run', 'Run Game'), disabled: !folderPath, onClick: handleRunGame },
  ];

  return (
    <div className="rounded-lg border border-accent/30 bg-bg-secondary shadow-sm overflow-visible">
      {/* Header */}
      <button type="button" onClick={() => setCollapsed(c => !c)}
        className="w-full flex items-center gap-2 px-3 py-2 bg-accent/5 border-b border-accent/20 hover:bg-accent/10 transition-colors rounded-t-lg"
        aria-expanded={!collapsed}>
        <Settings2 className="w-3.5 h-3.5 text-accent flex-shrink-0" />
        <span className="flex-1 text-left text-xs font-semibold text-accent tracking-wide uppercase">
          {t('instanceSettings.title', 'Game Settings')}
        </span>
        {collapsed ? <ChevronRight className="w-3.5 h-3.5 text-accent/60" /> : <ChevronDown className="w-3.5 h-3.5 text-accent/60" />}
      </button>

      {!collapsed && (
        <div className="p-3 space-y-3">

          {/* GameType select */}
          <div className="space-y-1">
            <p className="text-sm font-medium text-text-primary">
              {t('instanceSettings.gameType', 'Game Type')}
            </p>
            <div className="flex gap-1 flex-wrap">
              {GAME_TYPES.map(gt => (
                <button key={gt.value} type="button"
                  onClick={() => setType(gt.value)}
                  className={clsx(
                    'px-3 py-1.5 rounded-md text-sm transition-colors border',
                    gameType === gt.value
                      ? 'bg-accent text-white border-accent'
                      : 'bg-bg-primary text-text-secondary border-border hover:bg-bg-hover hover:text-text-primary',
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
              <input type="text" value={folderPath}
                onChange={e => setPath(e.target.value)}
                placeholder={t('options.folder.placeholder', 'Select a folder…')}
                className="flex-1 min-w-0 px-3 py-1.5 rounded-md bg-bg-primary border border-border text-sm text-text-primary placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-accent font-mono" />
              <DropdownMenu items={menuItems} />
            </div>
          </div>

        </div>
      )}
    </div>
  );
}
