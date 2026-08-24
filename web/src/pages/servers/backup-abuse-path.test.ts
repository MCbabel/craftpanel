import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

const ROOT = resolve(import.meta.dirname, '../../../..')

const CHAIN: { link: string; file: string; needs: string[] }[] = [
	{
		link: 'The backups page reads the code Google\'s refusal comes back as',
		file: 'web/src/pages/servers/Backups.vue',
		needs: ["last.error?.code === 'drive_abuse_blocked'", 'function abuseBlocked('],
	},
	{
		link: 'It warns the person before it offers them anything',
		file: 'web/src/pages/servers/Backups.vue',
		needs: [
			'abuseBlocked(backup)',
			'messages.abuseHeader',
			'messages.abuseWarning',
			'if (abuseBlocked(backup)) return `${why} ${formatMessage(messages.abuseWarning)}`.trim()',
		],
	},
	{
		link: 'And only then does a button of its own carry the acknowledgement',
		file: 'web/src/pages/servers/Backups.vue',
		needs: [
			'@click="retry(backup.id, true)"',
			'messages.abuseFetchAnyway',
			'async function retry(backupId: string, acknowledgeAbuse = false)',
			'acknowledgeAbuse ? { acknowledge_abuse: true } : {}',
		],
	},
	{
		link: 'The client puts it in the query of 10.7',
		file: 'web/src/api/client.ts',
		needs: [
			'export type RetryBackupQuery = { acknowledge_abuse?: boolean }',
			'query: RetryBackupQuery = {},',
		],
	},
	{
		link: 'The panel asks Google for it only when a person said so',
		file: 'crates/craftpanel/src/drive/files.rs',
		needs: ['if fetch.acknowledge_abuse { query.push(("acknowledgeAbuse", "true")); }'],
	},
	{
		link: 'And both catalogues carry the warning',
		file: 'web/src/locales/de-DE/index.json',
		needs: ['craftpanel.backups.abuse-warning', 'craftpanel.backups.abuse-fetch-anyway'],
	},
]

function collapsed(source: string): string {
	return source.replace(/\s+/g, ' ')
}

describe('The way from a file Google calls harmful to a download nobody was tricked into', () => {
	it.each(CHAIN)('$link', ({ file, needs }) => {
		const source = collapsed(readFileSync(resolve(ROOT, file), 'utf8'))
		for (const needle of needs) {
			expect(source, `${file} no longer carries ${needle}`).toContain(collapsed(needle))
		}
	})

	it('never sets acknowledgeAbuse on its own', () => {
		const source = collapsed(
			readFileSync(resolve(ROOT, 'crates/craftpanel/src/drive/files.rs'), 'utf8'),
		)
		const uses = source.split('acknowledgeAbuse').length - 1
		expect(uses, 'a second place sets the parameter, and one of them may be silent').toBe(1)
	})
})
