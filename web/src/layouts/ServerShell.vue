<template>
	<div v-if="loading" class="relative grid place-items-center py-16">
		<LoadingIndicator />
	</div>

	<div v-else-if="failure" class="flex justify-center py-16">
		<ErrorInformationCard v-bind="failure" />
	</div>

	<ServerFrame v-else-if="server" :key="server.id" :server="server" />
</template>

<script setup lang="ts">
import {
	LeftArrowIcon,
	LockIcon,
	SearchIcon,
	TriangleAlertIcon,
	UpdatedIcon,
} from '@modrinth/assets'
import {
	commonMessages,
	defineMessages,
	ErrorInformationCard,
	LoadingIndicator,
	useVIntl,
} from '@modrinth/ui'
import { type Component, computed, onScopeDispose, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { api, isApiRequestError, type Server } from '@/api'
import { provideModrinthHost } from '@/composables/modrinth-host'
import ServerFrame from '@/layouts/ServerFrame.vue'

interface Failure {
	title: string
	description: string
	icon: Component
	action: { label: string; onClick: () => void; icon: Component }
	errorDetails?: { label: string; value: string; type: 'inline' }[]
}

const { formatMessage } = useVIntl()
const route = useRoute()
const router = useRouter()

provideModrinthHost()

const messages = defineMessages({
	missingTitle: {
		id: 'panel.server.missing.title',
		defaultMessage: 'Server not found',
	},
	missingDescription: {
		id: 'panel.server.missing.description',
		defaultMessage: 'This server does not exist, or it has been deleted.',
	},
	forbiddenTitle: {
		id: 'panel.server.forbidden.title',
		defaultMessage: 'No access to this server',
	},
	forbiddenDescription: {
		id: 'panel.server.forbidden.description',
		defaultMessage: 'Your access to this server was removed.',
	},
	unreachableTitle: {
		id: 'panel.server.unreachable.title',
		defaultMessage: 'The server could not be loaded',
	},
	unreachableDescription: {
		id: 'panel.server.unreachable.description',
		defaultMessage: 'The panel did not answer. Try again in a moment.',
	},
	back: {
		id: 'panel.server.error.back',
		defaultMessage: 'Back to servers',
	},
})

const serverId = computed(() => String(route.params.id ?? ''))
const server = ref<Server | null>(null)
const failure = ref<Failure | null>(null)
const loading = ref(true)

let reading: AbortController | null = null

function toServerList(): void {
	void router.push({ name: 'servers' })
}

function failureFor(error: unknown): Failure {
	const back = {
		label: formatMessage(messages.back),
		onClick: toServerList,
		icon: LeftArrowIcon,
	}

	if (isApiRequestError(error) && error.status === 404) {
		return {
			title: formatMessage(messages.missingTitle),
			description: formatMessage(messages.missingDescription),
			icon: SearchIcon,
			action: back,
		}
	}
	if (isApiRequestError(error) && error.status === 403) {
		return {
			title: formatMessage(messages.forbiddenTitle),
			description: formatMessage(messages.forbiddenDescription),
			icon: LockIcon,
			action: back,
		}
	}

	return {
		title: formatMessage(messages.unreachableTitle),
		description: formatMessage(messages.unreachableDescription),
		icon: TriangleAlertIcon,
		action: {
			label: formatMessage(commonMessages.retryButton),
			onClick: () => void load(),
			icon: UpdatedIcon,
		},
		errorDetails: [
			{
				label: formatMessage(commonMessages.errorLabel),
				value: isApiRequestError(error) ? `${error.code}: ${error.message}` : String(error),
				type: 'inline',
			},
		],
	}
}

async function load(): Promise<void> {
	reading?.abort()
	const current = new AbortController()
	reading = current

	loading.value = true
	failure.value = null
	try {
		server.value = await api.servers.get(serverId.value, { signal: current.signal })
	} catch (error) {
		if (current.signal.aborted) return
		server.value = null
		failure.value = failureFor(error)
	} finally {
		if (!current.signal.aborted) loading.value = false
	}
}

watch(serverId, () => void load(), { immediate: true })

onScopeDispose(() => reading?.abort())
</script>
