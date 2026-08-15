import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

const ROOT = resolve(import.meta.dirname, '../../../..')

const CHAIN: { link: string; file: string; needs: string[] }[] = [
	{
		link: 'The provider reads the config folder (7.3), once per folder',
		file: 'web/src/providers/content-manager.ts',
		needs: [
			"from '@/providers/config-location'",
			'const root = configRoot(fetched.content_type, item.file_path)',
			'client.files.listAll(serverId, root, { signal: lifetime.signal })',
			'hasErrorCode(cause, "not_found") ? [] : null',
		],
	},
	{
		link: 'And hangs the button of the row on the result',
		file: 'web/src/providers/content-manager.ts',
		needs: [
			'const spot = configLocation(',
			'if (spot === null) return []',
			'label: formatMessage(messages.configNone)',
			'disabled: true',
			'to: options.fileLink(spot.path, spot.kind === "own" ? spot.editing : undefined)',
			'getOverflowOptions: (item) => configOptions(item.id)',
		],
	},
	{
		link: 'The content page says where the link leads',
		file: 'web/src/pages/servers/Content.vue',
		needs: [
			'fileLink: (path, editing) => ({',
			'name: "server-files"',
			'query: editing === undefined ? { path } : { path, editing }',
		],
	},
	{
		link: 'The router knows this name',
		file: 'web/src/router.ts',
		needs: ["name: 'server-files'", "import('@/pages/servers/Files.vue')"],
	},
	{
		link: 'And the files page goes to the folder from the address, not to the root directory',
		file: 'web/src/pages/servers/Files.vue',
		needs: [
			'const folder = folderToOpen(query)',
			'else if (folder !== null) files.context.navigateTo(folder)',
		],
	},
	{
		link: 'Modrinth\'s row fetches the actions from us',
		file: 'vendor/modrinth/ui/src/layouts/shared/content-tab/layout.vue',
		needs: ['overflowOptions: ctx.getOverflowOptions?.(item)', ':items="tableItems"'],
	},
	{
		link: 'And passes them through into the menu of the row',
		file: 'vendor/modrinth/ui/src/layouts/shared/content-tab/components/ContentCardTable.vue',
		needs: [':overflow-options="item.overflowOptions"'],
	},
	{
		link: 'There stands the button, and a link really does become a link',
		file: 'vendor/modrinth/ui/src/layouts/shared/content-tab/components/ContentCardItem.vue',
		needs: ['v-if="overflowOptions?.length"', ':options="overflowOptions"'],
	},
	{
		link: 'Modrinth\'s menu turns `to` into a route link and locks a locked entry',
		file: 'vendor/modrinth/ui/src/components/base/buttons/TeleportOverflowMenu.vue',
		needs: ['isLink(option) && option.to !== undefined && !option.disabled', ':to="option.to"'],
	},
]

function collapsed(source: string): string {
	return source.replace(/\s+/g, ' ').replace(/'/g, '"')
}

describe('The way from the row into the config folder', () => {
	it.each(CHAIN)('$link', ({ file, needs }) => {
		const source = collapsed(readFileSync(resolve(ROOT, file), 'utf8'))
		for (const needle of needs) {
			expect(source, `${file} no longer carries ${needle}`).toContain(collapsed(needle))
		}
	})
})
