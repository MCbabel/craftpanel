import { computed, ref } from 'vue'

import { api, isApiRequestError, type Me } from '@/api'

const user = ref<Me | null>(null)
const settled = ref(false)
let inflight: Promise<Me | null> | null = null

const isAuthenticated = computed(() => user.value !== null)
const isAdmin = computed(() => user.value?.panel_role === 'admin')
const mustChangePassword = computed(() => user.value?.must_change_password === true)

async function fetchMe(): Promise<Me | null> {
	try {
		user.value = await api.auth.me()
		settled.value = true
	} catch (error) {
		user.value = null
		settled.value = isApiRequestError(error) && error.status === 401
	}
	return user.value
}

function resolve(): Promise<Me | null> {
	if (settled.value) return Promise.resolve(user.value)
	inflight ??= fetchMe().finally(() => {
		inflight = null
	})
	return inflight
}

function refresh(): Promise<Me | null> {
	return fetchMe()
}

function adopt(me: Me): void {
	user.value = me
	settled.value = true
}

function forget(): void {
	user.value = null
	settled.value = true
}

async function signOut(): Promise<void> {
	await api.auth.logout().catch(() => undefined)
	forget()
}

export function useSession() {
	return {
		user,
		isAuthenticated,
		isAdmin,
		mustChangePassword,
		resolve,
		refresh,
		adopt,
		forget,
		signOut,
	}
}
