import type { Tags } from '@modrinth/ui'
import { describe, expect, it } from 'vitest'

import { loaderFacet } from './loader-facets'

const tags = {
	gameVersions: [],
	categories: [],
	loaders: [
		{ name: 'bukkit', supported_project_types: ['plugin'] },
		{ name: 'spigot', supported_project_types: ['plugin'] },
		{ name: 'paper', supported_project_types: ['plugin'] },
		{ name: 'purpur', supported_project_types: ['plugin'] },
		{ name: 'folia', supported_project_types: ['plugin'] },
		{ name: 'velocity', supported_project_types: ['plugin'] },
		{ name: 'fabric', supported_project_types: ['mod', 'modpack'] },
		{ name: 'neoforge', supported_project_types: ['mod', 'modpack'] },
	],
} as unknown as Tags

describe('loaderFacet', () => {
	it('preselects Paper only on a Paper server', () => {
		expect(loaderFacet('plugin', 'paper', tags)).toEqual([
			{ type: 'plugin_loader', option: 'paper' },
		])
	})

	it('does not bring the Bukkit relatives along', () => {
		const options = loaderFacet('plugin', 'paper', tags).map((filter) => filter.option)
		expect(options).not.toContain('bukkit')
		expect(options).not.toContain('spigot')
		expect(options).not.toContain('purpur')
	})

	it('tells the servers of the Bukkit family apart', () => {
		expect(loaderFacet('plugin', 'purpur', tags)).toEqual([
			{ type: 'plugin_loader', option: 'purpur' },
		])
		expect(loaderFacet('plugin', 'folia', tags)).toEqual([
			{ type: 'plugin_loader', option: 'folia' },
		])
		expect(loaderFacet('plugin', 'spigot', tags)).toEqual([
			{ type: 'plugin_loader', option: 'spigot' },
		])
	})

	it('puts a proxy into the platform filter', () => {
		expect(loaderFacet('plugin', 'velocity', tags)).toEqual([
			{ type: 'plugin_platform', option: 'velocity' },
		])
	})

	it('preselects the mod loader for mods', () => {
		expect(loaderFacet('mod', 'fabric', tags)).toEqual([{ type: 'mod_loader', option: 'fabric' }])
		expect(loaderFacet('mod', 'neoforge', tags)).toEqual([
			{ type: 'mod_loader', option: 'neoforge' },
		])
	})

	it('stays empty where Modrinth does not know the loader', () => {
		expect(loaderFacet('plugin', 'leaf', tags)).toEqual([])
		expect(loaderFacet('mod', 'paper', tags)).toEqual([])
	})

	it('filters nothing for types without a loader', () => {
		expect(loaderFacet('datapack', 'vanilla', tags)).toEqual([])
		expect(loaderFacet('plugin', null, tags)).toEqual([])
	})
})
