import type { ContentListResponse } from '@/api'

export interface InstalledProjects {
	installed: ReadonlySet<string>
	installing: ReadonlySet<string>
}

export function installedProjects(list: ContentListResponse | null): InstalledProjects {
	const installed = new Set<string>()
	const installing = new Set<string>()
	for (const item of list?.items ?? []) {
		if (!item.project_id) continue
		if (item.installing) installing.add(item.project_id)
		else installed.add(item.project_id)
	}
	for (const id of installing) installed.delete(id)
	if (list?.modpack?.project_id) installed.add(list.modpack.project_id)
	return { installed, installing }
}

export function installedFacts(projects: InstalledProjects): string {
	const sorted = (ids: ReadonlySet<string>) => [...ids].sort().join(',')
	return `${sorted(projects.installed)}|${sorted(projects.installing)}`
}

export type InstallState = 'available' | 'selected' | 'installing' | 'installed'

export function installState(
	projectId: string,
	projects: InstalledProjects,
	selected: boolean,
): InstallState {
	if (projects.installing.has(projectId)) return 'installing'
	if (projects.installed.has(projectId)) return 'installed'
	return selected ? 'selected' : 'available'
}

export function isSelectable(state: InstallState): boolean {
	return state === 'available' || state === 'selected'
}

export function stillSelectable<T>(
	selection: ReadonlyMap<string, T>,
	projects: InstalledProjects,
): Map<string, T> {
	const kept = new Map<string, T>()
	for (const [id, value] of selection) {
		if (isSelectable(installState(id, projects, true))) kept.set(id, value)
	}
	return kept
}
