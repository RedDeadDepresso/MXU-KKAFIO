import { useState, useEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  CheckSquare,
  Square,
  ChevronsUpDown,
  ChevronsDownUp,
  Plus,
  Play,
  StopCircle,
  Loader2,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '@/stores/appStore';
import { isTaskCompatible } from '@/stores/helpers';
import clsx from 'clsx';
import { loggers } from '@/utils';
import { SchedulePanel } from './SchedulePanel';
import { ScheduleButton } from './toolbar/ScheduleButton';
import { scheduleService } from '@/services/scheduleService';
import { isTauri } from '@/utils/paths';

const log = loggers.task;

interface ToolbarProps {
  showAddPanel: boolean;
  onToggleAddPanel: () => void;
  className?: string;
}

export function Toolbar({ showAddPanel, onToggleAddPanel, className }: ToolbarProps) {
  const { t } = useTranslation();
  const {
    getActiveInstance,
    selectAllTasks,
    collapseAllTasks,
    projectInterface,
    basePath,
    selectedController,
    selectedResource,
    scheduleExecutions,
    addLog,
  } = useAppStore();

  const [isRunning, setIsRunning] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [showSchedulePanel, setShowSchedulePanel] = useState(false);

  const instance = getActiveInstance();
  const tasks = instance?.selectedTasks || [];
  const anyExpanded = tasks.some((t) => t.expanded);
  const instanceId = instance?.id || '';

  const currentControllerName =
    selectedController[instanceId] ||
    instance?.controllerName ||
    projectInterface?.controller[0]?.name;
  const currentResourceName =
    selectedResource[instanceId] || instance?.resourceName || projectInterface?.resource[0]?.name;

  const allEnabled = useMemo(() => {
    if (tasks.length === 0) return false;
    const compatibleTasks = tasks.filter((t) => {
      const taskDef = projectInterface?.task.find((td) => td.name === t.taskName);
      return isTaskCompatible(taskDef, currentControllerName, currentResourceName);
    });
    return compatibleTasks.length > 0 && compatibleTasks.every((t) => t.enabled);
  }, [tasks, projectInterface, currentControllerName, currentResourceName]);

  const canRun = tasks.some((t) => t.enabled);
  const isDisabled = (tasks.length === 0 || !canRun) && !isRunning;
  const selectedTaskCount = useMemo(() => tasks.filter((t) => t.enabled).length, [tasks]);

  const handleSelectAll = () => {
    if (!instance) return;
    selectAllTasks(instance.id, !allEnabled);
  };

  const handleCollapseAll = () => {
    if (!instance) return;
    collapseAllTasks(instance.id, !anyExpanded);
  };

  // Poll kkafio_is_running every second to keep button state in sync
  useEffect(() => {
    if (!isTauri()) return;
    const poll = async () => {
      try {
        const running = await invoke<boolean>('kkafio_is_running');
        setIsRunning(running);
        if (!running) {
          setIsStarting(false);
          setIsStopping(false);
        }
      } catch {
        // ignore
      }
    };
    poll();
    const id = window.setInterval(poll, 1000);
    return () => window.clearInterval(id);
  }, []);

  const handleStart = useCallback(async () => {
    if (!canRun) return;

    const cwd = basePath;
    if (!cwd) {
      if (instanceId) addLog(instanceId, { type: 'error', message: 'No base path set — cannot start KKAFIO CLI.' });
      return;
    }

    // Compute the zero-based index of the active instance so the CLI knows
    // which instance block to read from the MXU config file.
    const storeState = useAppStore.getState();
    const instanceIndex = storeState.instances.findIndex((i) => i.id === instanceId);
    const safeIndex = instanceIndex >= 0 ? instanceIndex : 0;

    setIsStarting(true);
    if (instanceId) addLog(instanceId, { type: 'info', message: t('taskList.startingTasks') });

    try {
      await invoke('kkafio_start', { cwd, instanceIndex: safeIndex });
      setIsRunning(true);
      if (instanceId) addLog(instanceId, { type: 'success', message: t('taskList.startTasks') });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      log.error('kkafio_start failed:', err);
      if (instanceId) addLog(instanceId, { type: 'error', message: msg });
      setIsRunning(false);
    } finally {
      setIsStarting(false);
    }
  }, [canRun, basePath, instanceId, addLog, t]);

  const handleStop = useCallback(async () => {
    setIsStopping(true);
    if (instanceId) addLog(instanceId, { type: 'info', message: t('taskList.stoppingTasks') });
    try {
      await invoke('kkafio_stop');
      setIsRunning(false);
      if (instanceId) addLog(instanceId, { type: 'success', message: t('taskList.stopTasks') });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      log.error('kkafio_stop failed:', err);
      if (instanceId) addLog(instanceId, { type: 'error', message: msg });
    } finally {
      setIsStopping(false);
    }
  }, [instanceId, addLog, t]);

  const handleStartStop = () => {
    if (isRunning) handleStop();
    else handleStart();
  };

  // Schedule service integration using setTriggerCallback pattern
  useEffect(() => {
    scheduleService.setTriggerCallback(async (instance) => {
      // In KKAFIO mode we just start/stop the CLI for the active instance.
      // The ScheduleService calls us with the scheduled instance; we check
      // whether it matches the currently active one before acting.
      const activeId = useAppStore.getState().activeInstanceId;
      if (instance.id !== activeId) return false;
      await handleStart();
      return true;
    });
    return () => {
      scheduleService.setTriggerCallback(null);
    };
  }, [handleStart]);

  // Global hotkey support — F10 start, F11 stop
  useEffect(() => {
    const onStart = async (evt: Event) => {
      const currentInstance = useAppStore.getState().getActiveInstance();
      if (!currentInstance || isRunning) return;
      const combo = ((evt as CustomEvent)?.detail as { combo?: string })?.combo || '';
      addLog(currentInstance.id, {
        type: 'info',
        message: t('logs.messages.hotkeyDetected', { combo, action: t('logs.messages.hotkeyActionStart') }),
      });
      await handleStart();
    };
    const onStop = async (evt: Event) => {
      if (!isRunning || isStopping) return;
      const combo = ((evt as CustomEvent)?.detail as { combo?: string })?.combo || '';
      if (instanceId) addLog(instanceId, {
        type: 'info',
        message: t('logs.messages.hotkeyDetected', { combo, action: t('logs.messages.hotkeyActionStop') }),
      });
      await handleStop();
    };
    document.addEventListener('mxu-start-tasks', onStart);
    document.addEventListener('mxu-stop-tasks', onStop);
    return () => {
      document.removeEventListener('mxu-start-tasks', onStart);
      document.removeEventListener('mxu-stop-tasks', onStop);
    };
  }, [isRunning, isStopping, handleStart, handleStop, addLog, instanceId, t]);

  return (
    <div
      className={clsx(
        'flex items-center justify-between px-3 py-2 bg-bg-secondary border-t border-border',
        className,
      )}
    >
      {/* Left toolbar buttons */}
      <div className="flex items-center gap-1">
        <button
          onClick={handleSelectAll}
          disabled={tasks.length === 0}
          className={clsx(
            'flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-sm transition-colors',
            tasks.length === 0
              ? 'text-text-muted cursor-not-allowed'
              : 'text-text-secondary hover:bg-bg-hover hover:text-text-primary',
          )}
          title={allEnabled ? t('taskList.deselectAll') : t('taskList.selectAll')}
        >
          {allEnabled ? <CheckSquare className="w-4 h-4" /> : <Square className="w-4 h-4" />}
          <span className="hidden sm:inline">
            {allEnabled ? t('taskList.deselectAll') : t('taskList.selectAll')}
          </span>
        </button>

        <button
          onClick={handleCollapseAll}
          disabled={tasks.length === 0}
          className={clsx(
            'flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-sm transition-colors',
            tasks.length === 0
              ? 'text-text-muted cursor-not-allowed'
              : 'text-text-secondary hover:bg-bg-hover hover:text-text-primary',
          )}
          title={anyExpanded ? t('taskList.collapseAll') : t('taskList.expandAll')}
        >
          {anyExpanded ? <ChevronsDownUp className="w-4 h-4" /> : <ChevronsUpDown className="w-4 h-4" />}
          <span className="hidden sm:inline">
            {anyExpanded ? t('taskList.collapseAll') : t('taskList.expandAll')}
          </span>
        </button>

        <button
          id="add-task-button"
          onClick={onToggleAddPanel}
          className={clsx(
            'flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-sm transition-colors',
            showAddPanel
              ? 'bg-accent/10 text-accent'
              : 'text-text-secondary hover:bg-bg-hover hover:text-text-primary',
          )}
          title={t('taskList.addTask')}
        >
          <Plus className="w-4 h-4" />
          <span className="hidden sm:inline">{t('taskList.addTask')}</span>
        </button>
      </div>

      {/* Right: schedule + start/stop */}
      <div className="flex items-center gap-2 relative">
        <ScheduleButton
          enabledCount={instance?.schedulePolicies?.filter((p) => p.enabled).length || 0}
          scheduleExecution={instance ? scheduleExecutions[instance.id] : null}
          showPanel={showSchedulePanel}
          onToggle={() => setShowSchedulePanel(!showSchedulePanel)}
        />
        {showSchedulePanel && instance && (
          <SchedulePanel instanceId={instance.id} onClose={() => setShowSchedulePanel(false)} />
        )}

        <div className="relative">
          <button
            data-role="start-stop-button"
            onClick={handleStartStop}
            disabled={isDisabled || isStopping || (isStarting && !isRunning)}
            className={clsx(
              'flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium transition-colors',
              isStopping
                ? 'bg-warning text-white'
                : isRunning
                  ? 'bg-error hover:bg-error/90 text-white'
                  : isStarting
                    ? 'bg-success text-white'
                    : isDisabled
                      ? 'bg-bg-active text-text-tertiary cursor-not-allowed'
                      : 'bg-accent hover:bg-accent-hover text-white',
            )}
          >
            {isStopping ? (
              <><Loader2 className="w-4 h-4 animate-spin" /><span>{t('taskList.stoppingTasks')}</span></>
            ) : isRunning ? (
              <><StopCircle className="w-4 h-4" /><span>{t('taskList.stopTasks')}</span></>
            ) : isStarting ? (
              <><Loader2 className="w-4 h-4 animate-spin" /><span>{t('taskList.startingTasks')}</span></>
            ) : (
              <><Play className="w-4 h-4" /><span>{t('taskList.startTasks')}</span></>
            )}
          </button>
          {/* 已选中任务数量徽章 */}
          {selectedTaskCount > 0 && !isRunning && !isStarting && !isStopping && (
            <span className="absolute -top-1 -right-1 min-w-[18px] h-[18px] px-1 flex items-center justify-center bg-accent text-white text-xs font-medium rounded-full ring-2 ring-bg-secondary">
              {selectedTaskCount}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
