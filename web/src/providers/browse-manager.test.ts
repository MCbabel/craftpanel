import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

const ROOT = resolve(import.meta.dirname, '../../..')

const CHAIN: { link: string; file: string; needs: string[] }[] = [
	{
		link: 'The browse page reads the state out of the content list (8.1)',
		file: 'web/src/providers/browse-manager.ts',
		needs: [
			"from '@/providers/installed-projects'",
			'const projects = computed(() => installedProjects(list.value))',
		],
	},
	{
		link: 'The button on the card names it and locks itself accordingly',
		file: 'web/src/providers/browse-manager.ts',
		needs: [
			'const state = installState(result.project_id, projects.value, selected)',
			'const blocked = !isSelectable(state) || installing.value || isBusy.value || !canWrite.value',
			'disabled: blocked',
			'state === "installed" ? commonMessages.installedLabel',
			'getCardActions: (result) => [installAction(result)]',
		],
	},
	{
		link: 'The multiple selection takes in nothing that is already there or loading',
		file: 'web/src/providers/browse-manager.ts',
		needs: [
			'if (!isSelectable(installState(result.project_id, projects.value, selected))) return',
		],
	},
	{
		link: 'And whatever is installed meanwhile drops out of the selection',
		file: 'web/src/providers/browse-manager.ts',
		needs: [
			'const kept = stillSelectable(selection.value, projects.value)',
			'if (kept.size !== selection.value.size) selection.value = kept',
			'adoptList(fetchedList)',
			'adoptList(fetched)',
		],
	},
	{
		link: 'The bar at the bottom shows exactly this selection and installs exactly it',
		file: 'web/src/providers/browse-manager.ts',
		needs: [
			'selectedProjects: Array.from(selection.value.values())',
			'const chosen = Array.from(selection.value.keys())',
		],
	},
	{
		link: 'Modrinth\'s result card fetches the actions from us and really does lock the button',
		file: 'vendor/modrinth/ui/src/layouts/shared/browse-tab/layout.vue',
		needs: [
			'v-for="action in ctx.getCardActions(result, ctx.projectType.value)"',
			':disabled="action.disabled"',
		],
	},
	{
		link: 'And Modrinth\'s bar at the bottom hangs on the same selection',
		file: 'vendor/modrinth/ui/src/layouts/shared/browse-tab/components/SelectedProjectsFloatingBar.vue',
		needs: [
			'installContext.value?.selectedProjects ?? []',
			'void installContext.value?.installSelected?.()',
		],
	},
]

function collapsed(source: string): string {
	return source.replace(/\s+/g, ' ').replace(/'/g, '"')
}

describe('The way from "already installed" to the locked button', () => {
	it.each(CHAIN)('$link', ({ file, needs }) => {
		const source = collapsed(readFileSync(resolve(ROOT, file), 'utf8'))
		for (const needle of needs) {
			expect(source, `${file} no longer carries ${needle}`).toContain(collapsed(needle))
		}
	})
})
