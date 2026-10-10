/** harness 的展示名与固定数据色。
 *  颜色跟随实体、不跟随排名：筛选后剩下的 harness 不会被重新上色。
 *  槽位顺序即堆叠顺序（相邻色已通过色觉障碍校验）。 */
const META: Record<string, { name: string; slot: number }> = {
  codex: { name: "Codex", slot: 1 },
  "claude-code": { name: "Claude Code", slot: 2 },
  opencode: { name: "OpenCode", slot: 3 },
  zcode: { name: "Zcode", slot: 4 },
  dsh: { name: "DeepSeek Harness", slot: 5 },
  generic: { name: "Generic", slot: 6 },
  "claude-desktop": { name: "Claude Desktop", slot: 7 },
  "kimi-code": { name: "Kimi Code", slot: 8 },
};

export function harnessName(id: string): string {
  return META[id]?.name ?? id;
}

export function harnessSlot(id: string): number {
  return META[id]?.slot ?? 8;
}

export function harnessColor(id: string): string {
  return `var(--s-${harnessSlot(id)})`;
}

/** 按固定槽位排序（图表堆叠、图例都用这个顺序） */
export function bySlot(a: string, b: string): number {
  return harnessSlot(a) - harnessSlot(b) || a.localeCompare(b);
}
