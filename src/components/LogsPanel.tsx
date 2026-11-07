import React, { useRef, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { Trash2, Copy, Archive, RotateCcw } from 'lucide-react';
import clsx from 'clsx';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore, type LogType } from '@/stores/appStore';
import { ContextMenu, useContextMenu, type MenuItem } from './ContextMenu';
import { isTauri } from '@/utils/paths';
import { useExportLogs } from '@/utils/useExportLogs';
import { ExportLogsModal } from './settings/ExportLogsModal';
import { useIsMobile } from '@/hooks/useIsMobile';
import { getCurrentLogFileName } from '@/utils/logger';
import { clearPersistedRuntimeLogs } from '@/utils/runtimeLogPersistence';

function formatLogTime(date: Date, locale?: string) {
  return date.toLocaleTimeString(locale || undefined, {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

export function LogsPanel() {
  const { t, i18n } = useTranslation();
  const isMobile = useIsMobile();
  const logsContainerRef = useRef<HTMLDivElement>(null);

  const {
    activeInstanceId,
    instanceLogs,
    autoClearLogsOnLaunch,
    clearLogs,
    setAutoClearLogsOnLaunch,
  } = useAppStore();
  const { state: menuState, show: showMenu, hide: hideMenu } = useContextMenu();
  const { exportModal, handleExportLogs, closeExportModal, openExportedFile } = useExportLogs();

  // 获取当前实例的日志
  const logs = activeInstanceId ? instanceLogs[activeInstanceId] || [] : [];

  useEffect(() => {
    const el = logsContainerRef.current;
    if (el) {
      el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    }
  }, [logs]);

  const clearLogFiles = useCallback(async () => {
    if (!isTauri()) return;
    try {
      await invoke<number>('clear_log_files', {
        excludeFileName: getCurrentLogFileName(),
      });
    } catch {
      // ignore cleanup errors
    }
  }, []);

  const handleClear = useCallback(() => {
    if (activeInstanceId) {
      clearLogs(activeInstanceId);
    }
    clearPersistedRuntimeLogs();
    clearLogFiles();
  }, [activeInstanceId, clearLogFiles, clearLogs]);

  const handleCopyAll = useCallback(() => {
    const text = logs
      .map((log) => `[${log.timestamp.toLocaleTimeString()}] ${log.message}`)
      .join('\n');
    navigator.clipboard.writeText(text);
  }, [logs]);

  // KKAFIO log colors — fixed, not theme-dependent
  // Mirrors Logger.statusColor: INFO=#2d8cf0, SUCCESS=#00c12b, ERROR=#ed3f14, WARNING/agent=#f90
  const getLogStyle = (type: LogType): React.CSSProperties => {
    switch (type) {
      case 'success': return { color: '#00c12b' };
      case 'error':   return { color: '#ed3f14' };
      case 'warning': return { color: '#f90' };
      case 'info':    return { color: '#2d8cf0' };
      case 'focus':   return { color: '#2d8cf0' };
      case 'agent':   return {}; // muted default
      default:        return {};
    }
  };

  // 右键菜单处理
  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();

      const menuItems: MenuItem[] = [
        {
          id: 'export-logs',
          label: t('debug.exportLogs'),
          icon: Archive,
          disabled: !isTauri(),
          onClick: handleExportLogs,
        },
        {
          id: 'copy',
          label: t('logs.copyAll'),
          icon: Copy,
          disabled: logs.length === 0,
          onClick: handleCopyAll,
        },
        {
          id: 'clear',
          label: t('logs.clear'),
          icon: Trash2,
          disabled: logs.length === 0,
          danger: true,
          onClick: handleClear,
        },
      ];

      showMenu(e, menuItems);
    },
    [
      t,
      logs.length,
      handleExportLogs,
      handleCopyAll,
      handleClear,
      showMenu,
    ],
  );

  return (
    <div
      id="logs-panel"
      className={clsx(
        'flex flex-col bg-bg-secondary rounded-lg ring-1 ring-inset ring-border overflow-hidden',
        // 2.5rem = TabBar h-10；若 TabBar 高度变更需同步更新
        isMobile ? 'h-[calc(100dvh-2.5rem)]' : 'flex-1 min-h-50',
      )}
    >
      {/* 标题栏 */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-border shrink-0 rounded-t-lg">
        <span className="text-sm font-medium text-text-primary">{t('logs.title')}</span>
        <div className="flex items-center gap-1">
          <button
            onClick={(e) => {
              e.stopPropagation();
              handleExportLogs();
            }}
            disabled={!isTauri() || (exportModal.show && exportModal.status === 'exporting')}
            className={clsx(
              'p-1 rounded-md transition-colors',
              !isTauri()
                ? 'text-text-muted cursor-not-allowed'
                : 'text-text-secondary hover:bg-bg-tertiary hover:text-text-primary',
            )}
            title={t('debug.exportLogs')}
          >
            <Archive className="w-3.5 h-3.5" />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              handleClear();
            }}
            disabled={!isTauri() && logs.length === 0}
            className={clsx(
              'p-1 rounded-md transition-colors',
              !isTauri() && logs.length === 0
                ? 'text-text-muted cursor-not-allowed'
                : 'text-text-secondary hover:bg-bg-tertiary hover:text-text-primary',
            )}
            title={t('logs.clear')}
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              setAutoClearLogsOnLaunch(!autoClearLogsOnLaunch);
            }}
            className={clsx(
              'p-1 rounded-md transition-colors',
              autoClearLogsOnLaunch
                ? 'text-accent bg-accent-light'
                : 'text-text-muted hover:bg-bg-tertiary hover:text-text-secondary',
            )}
            title={t('logs.autoClearOnLaunch')}
          >
            <RotateCcw className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* 日志内容 */}
      <div
        ref={logsContainerRef}
        className="flex-1 min-h-0 overflow-y-auto p-2.5 font-mono text-[12px] leading-5 bg-bg-tertiary"
        onContextMenu={handleContextMenu}
      >
        {logs.length === 0 ? (
          <div className="h-full flex items-center justify-center text-text-muted">
            {t('logs.noLogs')}
          </div>
        ) : (
          <>
            {logs.map((log) =>
              log.html ? (
                // 富文本内容（focus 消息支持 Markdown/HTML）
                <div
                  key={log.id}
                  className={clsx(
                    'py-1.5 px-2 rounded-md flex items-start gap-3',
                    'odd:bg-black/0 even:bg-black/5',
                    log.type === 'agent' && 'text-text-muted',
                  )}
                  style={getLogStyle(log.type)}
                >
                  <span className="text-text-muted/90 w-[72px] flex-shrink-0 tabular-nums text-[11px] leading-5">
                    {formatLogTime(log.timestamp, i18n.language)}
                  </span>
                  <span
                    className="min-w-0 flex-1 break-words whitespace-pre-wrap leading-5 focus-content"
                    dangerouslySetInnerHTML={{ __html: log.html }}
                  />
                </div>
              ) : (
                <div
                  key={log.id}
                  className={clsx(
                    'py-1.5 px-2 rounded-md flex items-start gap-3',
                    'odd:bg-black/0 even:bg-black/5',
                    log.type === 'agent' && 'text-text-muted',
                  )}
                  style={getLogStyle(log.type)}
                >
                  <span className="text-text-muted/90 w-[72px] flex-shrink-0 tabular-nums text-[11px] leading-5">
                    {formatLogTime(log.timestamp, i18n.language)}
                  </span>
                  <span className="min-w-0 flex-1 break-words whitespace-pre-wrap leading-5">
                    {log.message}
                  </span>
                </div>
              ),
            )}
          </>
        )}
      </div>

      {/* 右键菜单 */}
      {menuState.isOpen && (
        <ContextMenu items={menuState.items} position={menuState.position} onClose={hideMenu} />
      )}

      {/* 导出日志 Modal */}
      <ExportLogsModal
        show={exportModal.show}
        status={exportModal.status === 'idle' ? 'exporting' : exportModal.status}
        zipPath={exportModal.zipPath}
        error={exportModal.error}
        onClose={closeExportModal}
        onOpen={openExportedFile}
      />
    </div>
  );
}
