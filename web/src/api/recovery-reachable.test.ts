import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

import { recovery } from './recovery'

const ROOT = resolve(import.meta.dirname, '../../..')

const REACHED_FROM: Record<keyof typeof recovery, { page: string; by: string }> = {
	request: { page: 'web/src/pages/auth/ForgotPassword.vue', by: '@submit.prevent="submit"' },
	whose: { page: 'web/src/pages/auth/NewPassword.vue', by: 'onMounted(' },
	confirm: { page: 'web/src/pages/auth/NewPassword.vue', by: '@submit.prevent="submit"' },
	sendFor: { page: 'web/src/pages/admin/Users.vue', by: '@click="sendResetLink"' },
}

function read(page: string): string {
	return readFileSync(resolve(ROOT, page), 'utf8')
}

function templateOf(source: string): string {
	return source.slice(0, source.lastIndexOf('</template>'))
}

function owner(source: string, at: number): string {
	const anchors = [...source.matchAll(/^(?:async )?function (\w+)\(|^(onMounted)\(/gm)]
	const enclosing = anchors.filter((anchor) => (anchor.index ?? 0) < at).at(-1)
	return enclosing?.[1] ?? enclosing?.[2] ?? '(nothing)'
}

function triggered(by: string): string {
	return /="(\w+)"/.exec(by)?.[1] ?? by.replace('(', '')
}

describe('every call from section 21', () => {
	it('stands in this table, the way the client offers it', () => {
		expect(Object.keys(recovery).sort()).toEqual(Object.keys(REACHED_FROM).sort())
	})

	it('is made from a page and not only from a test', () => {
		for (const [call, { page }] of Object.entries(REACHED_FROM)) {
			expect(read(page), `${page} never makes recovery.${call}`).toContain(
				`recovery.${call}(`,
			)
		}
	})

	it('hangs on something a person sets off', () => {
		for (const [call, { page, by }] of Object.entries(REACHED_FROM)) {
			const source = read(page)
			const handler = owner(source, source.indexOf(`recovery.${call}(`))

			expect(triggered(by), `recovery.${call} sits in ${handler} of ${page}`).toBe(handler)
			expect(
				handler === 'onMounted' ? source : templateOf(source),
				`${page} never sets off ${handler}`,
			).toContain(by)
		}
	})
})
