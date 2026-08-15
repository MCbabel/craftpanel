import type { FilterValue, Tags } from '@modrinth/ui'
import { normalizeLoaderAlias } from '@modrinth/ui/src/utils/version-compatibility.ts'

const PLUGIN_PLATFORMS = ['bungeecord', 'waterfall', 'velocity', 'geyser']

export function loaderFacet(projectType: string, loader: string | null, tags: Tags): FilterValue[] {
	if (!loader || (projectType !== 'mod' && projectType !== 'plugin')) return []

	const wanted = normalizeLoaderAlias(loader)
	const known = tags.loaders.find(
		(tag) =>
			normalizeLoaderAlias(tag.name) === wanted &&
			tag.supported_project_types.includes(projectType),
	)
	if (!known) return []

	return [
		{
			type:
				projectType === 'plugin' && PLUGIN_PLATFORMS.includes(known.name)
					? 'plugin_platform'
					: `${projectType}_loader`,
			option: known.name,
		},
	]
}
