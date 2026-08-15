import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

const ROOT = resolve(import.meta.dirname, '../../../..')
const PAGE = 'web/src/pages/admin/Users.vue'
const ENDPOINTS = 'crates/craftpanel/src/api/admin.rs'
const CLIENT = 'web/src/api/types.ts'

const CREATE = 'api.admin.createUser('
const UPDATE = 'api.admin.updateUser('

type Reached = { control: string; call: string }

const REACHED_FROM: Record<string, Record<string, Reached>> = {
	CreateUserRequest: {
		username: { control: 'id="create-username"', call: CREATE },
		email: { control: 'id="create-email"', call: CREATE },
		panel_role: { control: 'name="create-role"', call: CREATE },
		must_change_password: { control: 'id="create-must-change"', call: CREATE },
		limits: { control: 'v-model="createLimits"', call: CREATE },
		password: { control: ':text="revealedPassword"', call: CREATE },
	},
	UpdateUserRequest: {
		username: { control: 'id="edit-username"', call: UPDATE },
		email: { control: 'id="edit-email"', call: UPDATE },
		panel_role: { control: 'name="edit-role"', call: UPDATE },
		must_change_password: { control: 'id="edit-must-change"', call: UPDATE },
		password: { control: '@click="resetPassword"', call: UPDATE },
	},
}

function read(file: string): string {
	return readFileSync(resolve(ROOT, file), 'utf8')
}

function block(source: string, opening: string): string {
	const start = source.indexOf(opening)
	expect(start, `${opening} does not stand in the file`).toBeGreaterThan(-1)
	const from = start + opening.length
	return source.slice(from, source.indexOf('\n}', from))
}

function fieldsOfStruct(name: string): string[] {
	const body = block(read(ENDPOINTS), `struct ${name} {`)
	return [...body.matchAll(/^ {4}(?:pub(?:\([\w:]+\))? )?(\w+):/gm)].map((field) => field[1])
}

function fieldsOfInterface(name: string): string[] {
	const body = block(read(CLIENT), `export interface ${name} {`)
	return [...body.matchAll(/^\t(?:readonly )?(\w+)\??:/gm)].map((field) => field[1])
}

function templateOf(source: string): string {
	return source.slice(0, source.lastIndexOf('</template>'))
}

function bodiesPassedTo(source: string, call: string): string[] {
	const bodies: string[] = []
	for (let at = source.indexOf(call); at !== -1; at = source.indexOf(call, at + 1)) {
		let depth = 0
		let end = at + call.length - 1
		do {
			const char = source[end]
			if (char === '(' || char === '{') depth += 1
			if (char === ')' || char === '}') depth -= 1
			end += 1
		} while (depth > 0 && end < source.length)
		bodies.push(source.slice(at, end).replaceAll(/\/\/[^\n]*/g, ''))
	}
	expect(bodies.length, `${call} is never made`).toBeGreaterThan(0)
	return bodies
}

function carries(body: string, field: string): boolean {
	return new RegExp(`\\b${field}\\b\\s*[:,}]`).test(body)
}

describe.each(Object.keys(REACHED_FROM))('the fields of %s', (request) => {
	const expected = REACHED_FROM[request]

	it('stand in this table, the way the endpoint takes them', () => {
		expect(Object.keys(expected).sort()).toEqual(fieldsOfStruct(request).sort())
	})

	it('are known to the client under exactly the same names', () => {
		expect(fieldsOfInterface(request).sort()).toEqual(fieldsOfStruct(request).sort())
	})

	it('each have a control in the template', () => {
		const template = templateOf(read(PAGE))
		for (const [field, { control }] of Object.entries(expected)) {
			expect(template, `${PAGE} has no control for ${field} (${control})`).toContain(
				control,
			)
		}
	})

	it('travel along from there in the body of the call', () => {
		const source = read(PAGE)
		for (const [field, { call }] of Object.entries(expected)) {
			const bodies = bodiesPassedTo(source, call)
			expect(
				bodies.some((body) => carries(body, field)),
				`${call} never sends ${field} along`,
			).toBe(true)
		}
	})
})
