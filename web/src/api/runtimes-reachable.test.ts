import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

import { api } from './client'

const ROOT = resolve(import.meta.dirname, '../../..')
const PAGE = 'web/src/pages/admin/Runtimes.vue'

const REACHED_FROM = {
	javaRuntimes: { handler: 'load', by: 'onMounted(() => void load())' },
	fetchJavaRuntime: { handler: 'fetchOne', by: '@click="fetchOne(entry)"' },
	removeJavaRuntime: { handler: 'removeOne', by: '@click="removeOne"' },
} as const

function read(path: string): string {
	return readFileSync(resolve(ROOT, path), 'utf8')
}

function owner(source: string, at: number): string {
	const anchors = [...source.matchAll(/^(?:async )?function (\w+)\(/gm)]
	return anchors.filter((anchor) => (anchor.index ?? 0) < at).at(-1)?.[1] ?? '(nothing)'
}

describe('every call about the Java runtimes', () => {
	it('is offered by the client the page imports', () => {
		for (const call of Object.keys(REACHED_FROM)) {
			expect(typeof (api.admin as Record<string, unknown>)[call], call).toBe('function')
		}
	})

	it('is made from the page and not only from a test', () => {
		const source = read(PAGE)
		for (const call of Object.keys(REACHED_FROM)) {
			expect(source, `${PAGE} never makes api.admin.${call}`).toContain(`api.admin.${call}(`)
		}
	})

	it('hangs on something a person sets off', () => {
		const source = read(PAGE)
		for (const [call, { handler, by }] of Object.entries(REACHED_FROM)) {
			expect(owner(source, source.indexOf(`api.admin.${call}(`)), call).toBe(handler)
			expect(source, `${PAGE} never sets off ${handler}`).toContain(by)
		}
	})
})

describe('the way to the page', () => {
	it('is a menu entry, not a path somebody has to know', () => {
		const routes = read('web/src/pages/admin/routes.ts')
		expect(routes).toContain("name: 'admin-runtimes'")
		expect(routes).toContain("path: 'admin/runtimes'")
		expect(routes).toContain('label: messages.runtimes')
	})
})

describe('the switch for fetching Java', () => {
	it('is a toggle on the panel settings page, bound to the field that is saved', () => {
		expect(read('web/src/pages/admin/settings/sections.ts')).toContain(
			"{ id: 'java', component: JavaSettings }",
		)
		expect(read('web/src/pages/admin/settings/Java.vue')).toContain(
			'v-model="settings.java_auto_install"',
		)
	})
})
