/**
 * 选项相关的工具函数
 */

import type { CaseItem, OptionValue, OrderedCheckboxOption } from '@/types/interface';

const YES_CASE_NAMES = ['Yes', 'yes', 'Y', 'y'] as const;

const NO_CASE_NAMES = ['No', 'no', 'N', 'n'] as const;

/**
 * 根据 Switch 的选中状态查找对应的 case
 * @param cases case 列表
 * @param isChecked switch 是否选中
 * @returns 匹配的 case，如果没有找到返回 undefined
 */
export function findSwitchCase(
  cases: CaseItem[] | undefined,
  isChecked: boolean,
): CaseItem | undefined {
  if (!cases) return undefined;

  const targetNames: readonly string[] = isChecked ? YES_CASE_NAMES : NO_CASE_NAMES;
  return cases.find((c) => targetNames.includes(c.name));
}

/**
 * 解析 ordered_checkbox 的当前顺序与选中状态。
 * - order：已保存的顺序（去掉不存在的 case、去重），新增的 case 按定义顺序追加到末尾
 * - checked：已保存的选中项（无保存值时使用 default_case），按 order 的顺序排列
 */
export function resolveOrderedCheckbox(
  optionDef: OrderedCheckboxOption,
  value?: OptionValue,
): { order: string[]; checked: string[] } {
  const names = optionDef.cases.map((c) => c.name);
  const valid = new Set(names);
  const stored = value?.type === 'ordered_checkbox' ? value : undefined;

  const seen = new Set<string>();
  const order: string[] = [];
  for (const name of stored?.order ?? []) {
    if (valid.has(name) && !seen.has(name)) {
      seen.add(name);
      order.push(name);
    }
  }
  for (const name of names) {
    if (!seen.has(name)) {
      seen.add(name);
      order.push(name);
    }
  }

  const checkedSet = new Set(stored ? stored.checked : (optionDef.default_case ?? []));
  return { order, checked: order.filter((name) => checkedSet.has(name)) };
}
