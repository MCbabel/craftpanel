export function updatesNeedRestart(
	running: boolean,
	items: readonly { has_update: boolean }[],
	modpackHasUpdate: boolean,
): boolean {
	if (!running) return false
	return modpackHasUpdate || items.some((item) => item.has_update)
}
