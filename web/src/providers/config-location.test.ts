import { describe, expect, it } from 'vitest'

import {
	type ConfigEntry,
	type ConfigItem,
	configLocation,
	configPath,
	configRoot,
} from './config-location'

const PLUGINS: ConfigEntry[] = [
	{ name: '.paper-remapped', type: 'directory' },
	{ name: 'Chunky', type: 'directory' },
	{ name: 'Chunky-Bukkit-1.4.40.jar', type: 'file' },
	{ name: 'ChunkyBorder', type: 'directory' },
	{ name: 'ChunkyBorder-1.2.13.jar', type: 'file' },
	{ name: 'LuckPerms', type: 'directory' },
	{ name: 'LuckPerms-Bukkit-5.5.71.jar', type: 'file' },
	{ name: 'PAPIProxyBridge-Bukkit-1.8.4.jar', type: 'file' },
	{ name: 'SuperHarvest', type: 'directory' },
	{ name: 'SuperHarvest.jar', type: 'file' },
	{ name: 'TAB', type: 'directory' },
	{ name: 'TAB v6.1.2 - Paper 1.20.5 - 1.21.4.jar', type: 'file' },
	{ name: 'Veinminer', type: 'directory' },
	{ name: 'ViaVersion', type: 'directory' },
	{ name: 'ViaVersion-5.11.0.jar', type: 'file' },
	{ name: 'WorldEdit', type: 'directory' },
	{ name: 'bStats', type: 'directory' },
	{ name: 'spark', type: 'directory' },
	{ name: 'veinminer-enchant-2.11.2+1.21.1.jar', type: 'file' },
	{ name: 'veinminer-paper-2.11.2+1.21.1.jar', type: 'file' },
	{ name: 'worldedit-bukkit-7.3.9.jar', type: 'file' },
]

const FRESH: ConfigEntry[] = PLUGINS.filter((entry) => entry.type === 'file')

function plugin(over: Partial<ConfigItem> = {}): ConfigItem {
	return {
		contentType: 'plugin',
		filePath: '/plugins/worldedit-bukkit-7.3.9.jar',
		fileName: 'worldedit-bukkit-7.3.9.jar',
		title: 'WorldEdit',
		slug: 'worldedit',
		...over,
	}
}

const hasRun = new Map([['/plugins', PLUGINS]])

describe('configRoot', () => {
	it('takes the folder its jar sits in for a plugin', () => {
		expect(configRoot('plugin', '/plugins/worldedit-bukkit-7.3.9.jar')).toBe('/plugins')
	})

	it('takes `config` for a mod, not `mods`', () => {
		expect(configRoot('mod', '/mods/sodium-0.5.8.jar')).toBe('/config')
	})

	it('makes the same folder out of a path without a leading slash', () => {
		expect(configRoot('plugin', 'plugins/worldedit.jar')).toBe('/plugins')
	})

	it('gives a datapack no place — it has no configuration', () => {
		expect(configRoot('datapack', '/world/datapacks/vanilla-tweaks.zip')).toBeNull()
		expect(configRoot('resourcepack', '/resourcepacks/faithful.zip')).toBeNull()
	})
})

describe('configLocation after the first start', () => {
	it('finds the folder the plugin created itself', () => {
		expect(configLocation(plugin(), hasRun)).toEqual({ kind: 'own', path: '/plugins/WorldEdit' })
	})

	it('finds it as well when only the title without spaces fits', () => {
		const spot = configLocation(
			plugin({
				filePath: '/plugins/ChunkyBorder-1.2.13.jar',
				fileName: 'ChunkyBorder-1.2.13.jar',
				title: 'Chunky Border',
				slug: 'chunkyborder',
			}),
			hasRun,
		)
		expect(spot).toEqual({ kind: 'own', path: '/plugins/ChunkyBorder' })
	})

	it('does not mix up two plugins whose name sits inside the other', () => {
		const chunky = plugin({
			filePath: '/plugins/Chunky-Bukkit-1.4.40.jar',
			fileName: 'Chunky-Bukkit-1.4.40.jar',
			title: 'Chunky',
			slug: 'chunky',
		})
		expect(configLocation(chunky, hasRun)).toEqual({ kind: 'own', path: '/plugins/Chunky' })

		const reversed = new Map([
			[
				'/plugins',
				[
					{ name: 'ChunkyBorder', type: 'directory' },
					{ name: 'Chunky', type: 'directory' },
				] as ConfigEntry[],
			],
		])
		expect(configLocation(chunky, reversed)).toEqual({ kind: 'own', path: '/plugins/Chunky' })

		const onlyForeign = new Map([
			['/plugins', [{ name: 'ChunkyBorder', type: 'directory' }] as ConfigEntry[]],
		])
		expect(configLocation(chunky, onlyForeign)).toEqual({ kind: 'shared', path: '/plugins' })
	})

	it('takes the title when the slug says something else', () => {
		const spot = configLocation(
			plugin({
				filePath: '/plugins/TAB v6.1.2 - Paper 1.20.5 - 1.21.4.jar',
				fileName: 'TAB v6.1.2 - Paper 1.20.5 - 1.21.4.jar',
				title: 'TAB',
				slug: 'tab-was-taken',
			}),
			hasRun,
		)
		expect(spot).toEqual({ kind: 'own', path: '/plugins/TAB' })
	})

	it('finds the folder by way of the name of the jar as well', () => {
		const spot = configLocation(
			plugin({
				filePath: '/plugins/SuperHarvest.jar',
				fileName: 'SuperHarvest.jar',
				title: null,
				slug: null,
			}),
			hasRun,
		)
		expect(spot).toEqual({ kind: 'own', path: '/plugins/SuperHarvest' })
	})

	it('names the shared folder when the plugin creates none of its own', () => {
		const spot = configLocation(
			plugin({
				filePath: '/plugins/PAPIProxyBridge-Bukkit-1.8.4.jar',
				fileName: 'PAPIProxyBridge-Bukkit-1.8.4.jar',
				title: 'PAPIProxyBridge',
				slug: 'papiproxybridge',
			}),
			hasRun,
		)
		expect(spot).toEqual({ kind: 'shared', path: '/plugins' })
	})

	it('points at the shared folder rather than at a foreign one', () => {
		const spot = configLocation(
			plugin({
				filePath: '/plugins/veinminer-enchant-2.11.2+1.21.1.jar',
				fileName: 'veinminer-enchant-2.11.2+1.21.1.jar',
				title: 'VeinMiner Enchantment',
				slug: 'veinminer-enchantment',
			}),
			hasRun,
		)
		expect(spot).toEqual({ kind: 'shared', path: '/plugins' })
	})

	it('reads the folder name without regard for upper and lower case', () => {
		const spot = configLocation(
			plugin({
				filePath: '/plugins/veinminer-paper-2.11.2+1.21.1.jar',
				fileName: 'veinminer-paper-2.11.2+1.21.1.jar',
				title: 'VeinMiner',
				slug: 'veinminer',
			}),
			hasRun,
		)
		expect(spot).toEqual({ kind: 'own', path: '/plugins/Veinminer' })
	})
})

describe('configLocation before the first start', () => {
	const fresh = new Map([['/plugins', FRESH]])

	it('says there is nothing yet instead of jumping into the jar folder', () => {
		expect(configLocation(plugin(), fresh)).toEqual({ kind: 'none', path: '/plugins' })
	})

	it('says the same when the folder does not exist at all', () => {
		expect(configLocation(plugin({ contentType: 'mod' }), new Map([['/config', []]]))).toEqual({
			kind: 'none',
			path: '/config',
		})
	})

	it('is not convinced by a jar that is switched off', () => {
		const spot = configLocation(
			plugin(),
			new Map([['/plugins', [{ name: 'worldedit-bukkit-7.3.9.jar.disabled', type: 'file' }]]]),
		)
		expect(spot).toEqual({ kind: 'none', path: '/plugins' })
	})
})

describe('configLocation for a mod', () => {
	it('opens the single file the mod wrote', () => {
		const spot = configLocation(
			{
				contentType: 'mod',
				filePath: '/mods/sodium-0.5.8.jar',
				fileName: 'sodium-0.5.8.jar',
				title: 'Sodium',
				slug: 'sodium',
			},
			new Map([
				[
					'/config',
					[
						{ name: 'sodium.json', type: 'file' },
						{ name: 'sodium-extra.json', type: 'file' },
					] as ConfigEntry[],
				],
			]),
		)
		expect(spot).toEqual({ kind: 'own', path: '/config', editing: 'sodium.json' })
		expect(configPath(spot!)).toBe('/config/sodium.json')
	})

	it('prefers the folder of its own to the single file', () => {
		const spot = configLocation(
			{
				contentType: 'mod',
				filePath: '/mods/sodium-0.5.8.jar',
				fileName: 'sodium-0.5.8.jar',
				title: 'Sodium',
				slug: 'sodium',
			},
			new Map([
				[
					'/config',
					[
						{ name: 'sodium.json', type: 'file' },
						{ name: 'sodium', type: 'directory' },
					] as ConfigEntry[],
				],
			]),
		)
		expect(spot).toEqual({ kind: 'own', path: '/config/sodium' })
	})
})

describe('configLocation without an answer', () => {
	it('says nothing while the folder is not read', () => {
		expect(configLocation(plugin(), new Map())).toBeNull()
	})

	it('says nothing about a datapack', () => {
		const spot = configLocation(
			plugin({
				contentType: 'datapack',
				filePath: '/world/datapacks/vanilla-tweaks.zip',
				fileName: 'vanilla-tweaks.zip',
			}),
			new Map([['/world/datapacks', PLUGINS]]),
		)
		expect(spot).toBeNull()
	})

	it('names the folder itself as the target for a folder', () => {
		expect(configPath({ kind: 'own', path: '/plugins/WorldEdit' })).toBe('/plugins/WorldEdit')
		expect(configPath({ kind: 'shared', path: '/plugins' })).toBe('/plugins')
	})
})
