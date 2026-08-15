import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

const ROOT = resolve(import.meta.dirname, '../../../..')

const CHAIN: { link: string; file: string; needs: string[] }[] = [
	{
		link: 'The backups page locks the trigger and says next to it why',
		file: 'web/src/pages/servers/Backups.vue',
		needs: ['cannotBackUp', ':disabled="createDisabled"', 'messages.driveBrokenHeader'],
	},
	{
		link: 'And it gives the owner the way to their account page',
		file: 'web/src/pages/servers/Backups.vue',
		needs: [
			'v-if="ownerCanConnect"',
			'const ownerCanConnect = computed(() => isOwner.value && needsOwnersDrive(target.value))',
			"router.push({ name: 'account' })",
			'messages.driveConnect',
		],
	},
	{
		link: 'The router knows this name, and it leads to the account page',
		file: 'web/src/router.ts',
		needs: ["name: 'account'", "import('@/pages/account/Account.vue')"],
	},
	{
		link: 'The account page shows the cards from `sections.ts`',
		file: 'web/src/pages/account/Account.vue',
		needs: ['v-for="section of accountSections"', "from './sections'"],
	},
	{
		link: 'And the Drive card stands in it',
		file: 'web/src/pages/account/sections.ts',
		needs: ["import AccountDrive from './Drive.vue'", "id: 'drive'"],
	},
	{
		link: 'The card has the button that begins the device flow (22.4)',
		file: 'web/src/pages/account/Drive.vue',
		needs: ['@click="beginLink"', 'drive.startLink()', 'link.verification_url'],
	},
	{
		link: 'And when it fails, the reason stands there — not just "denied" (22.5)',
		file: 'web/src/pages/account/Drive.vue',
		needs: ['view.lastFailure ?? formatMessage(messages.linkDenied)', 'view.lastFailure &&'],
	},
]

const TRIGGERS: { what: string; lock: string }[] = [
	{
		what: 'Create a backup',
		lock: 'const createDisabled = computed( () => !canManageBackups.value || cannotBackUp.value',
	},
	{
		what: 'Retry a failed one',
		lock: 'const retryDisabled = computed(() => !canManageBackups.value || cannotBackUp.value)',
	},
	{
		what: 'Switch the schedule on — switching it off stays possible',
		lock: '(cannotBackUp.value && !schedule.value.enabled)',
	},
]

function collapsed(source: string): string {
	return source.replace(/\s+/g, ' ')
}

describe('The way from "cannot back up" to the connected Drive', () => {
	it.each(CHAIN)('$link', ({ file, needs }) => {
		const source = readFileSync(resolve(ROOT, file), 'utf8')
		for (const needle of needs) {
			expect(source, `${file} no longer carries ${needle}`).toContain(needle)
		}
	})

	it.each(TRIGGERS)('is locked when nothing can succeed: $what', ({ lock }) => {
		const source = collapsed(readFileSync(resolve(ROOT, 'web/src/pages/servers/Backups.vue'), 'utf8'))
		expect(source, `this trigger no longer hangs on cannotBackUp`).toContain(collapsed(lock))
	})
})
