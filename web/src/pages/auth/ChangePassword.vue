<template>
	<div class="mx-auto flex min-h-full w-full max-w-[27rem] flex-col justify-center px-6 py-12">
		<Card class="!mb-0 flex flex-col gap-6">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
				<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
			</div>

			<Admonition
				v-if="mustChangePassword"
				type="warning"
				:body="formatMessage(messages.required)"
			/>

			<form class="flex flex-col gap-4" @submit.prevent="submit">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="current-password">
						{{ formatMessage(messages.currentPassword) }}
					</label>
					<StyledInput
						id="current-password"
						v-model="current"
						type="password"
						name="current-password"
						autocomplete="current-password"
						:disabled="busy"
						:input-attrs="{ autofocus: true }"
						wrapper-class="w-full"
					/>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="new-password">
						{{ formatMessage(messages.newPassword) }}
					</label>
					<StyledInput
						id="new-password"
						v-model="next"
						type="password"
						name="new-password"
						autocomplete="new-password"
						:disabled="busy"
						wrapper-class="w-full"
					/>
					<span class="text-sm text-secondary">
						{{ formatMessage(messages.lengthHint, { length: MINIMUM_LENGTH }) }}
					</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="confirm-password">
						{{ formatMessage(commonMessages.confirmPasswordLabel) }}
					</label>
					<StyledInput
						id="confirm-password"
						v-model="confirmation"
						type="password"
						name="confirm-password"
						autocomplete="new-password"
						:disabled="busy"
						wrapper-class="w-full"
					/>
				</div>

				<Admonition v-if="failure" type="critical" :body="failure" />

				<div class="flex flex-col gap-2">
					<Button
						native-type="submit"
						type="colored"
						color="brand"
						size="lg"
						:disabled="!current || !next || !confirmation"
						:loading="busy"
						class="!w-full !justify-center"
					>
						<SpinnerIcon v-if="busy" class="animate-spin" />
						{{ formatMessage(commonMessages.saveButton) }}
					</Button>

					<Button v-if="mustChangePassword" size="lg" class="!w-full !justify-center" @click="leave">
						<LogOutIcon />
						{{ formatMessage(commonMessages.signOutButton) }}
					</Button>
					<ButtonLink v-else :to="target" size="lg" class="!w-full !justify-center">
						{{ formatMessage(commonMessages.cancelButton) }}
					</ButtonLink>
				</div>
			</form>
		</Card>
	</div>
</template>

<script setup lang="ts">
import { LogOutIcon, SpinnerIcon } from '@modrinth/assets'
import {
	Admonition,
	Button,
	ButtonLink,
	Card,
	commonMessages,
	defineMessages,
	type MessageDescriptor,
	StyledInput,
	useVIntl,
} from '@modrinth/ui'
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

import { api, isApiRequestError } from '@/api'
import { useSession } from '@/composables/session'
import { internalPath } from '@/router'

const MINIMUM_LENGTH = 10

const { formatMessage } = useVIntl()
const route = useRoute()
const router = useRouter()
const { mustChangePassword, refresh, signOut } = useSession()

const messages = defineMessages({
	title: {
		id: 'auth.change-password.title',
		defaultMessage: 'Change password',
	},
	subtitle: {
		id: 'auth.change-password.subtitle',
		defaultMessage: 'Every other session of yours is signed out.',
	},
	required: {
		id: 'auth.change-password.required',
		defaultMessage: 'You need a new password before you can use the panel.',
	},
	currentPassword: {
		id: 'auth.change-password.current',
		defaultMessage: 'Current password',
	},
	newPassword: {
		id: 'auth.change-password.new',
		defaultMessage: 'New password',
	},
	lengthHint: {
		id: 'auth.change-password.length-hint',
		defaultMessage: 'At least {length} characters.',
	},
	mismatch: {
		id: 'auth.change-password.mismatch',
		defaultMessage: 'The two new passwords do not match.',
	},
	failed: {
		id: 'auth.change-password.failed',
		defaultMessage: 'The password could not be changed. Please try again.',
	},
})

const failures: Record<string, MessageDescriptor> = defineMessages({
	wrong_password: {
		id: 'auth.change-password.error.wrong-password',
		defaultMessage: 'The current password is incorrect.',
	},
	weak_password: {
		id: 'auth.change-password.error.weak-password',
		defaultMessage: 'The new password is too short.',
	},
	network_unreachable: {
		id: 'auth.change-password.error.network-unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
})

const current = ref('')
const next = ref('')
const confirmation = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)

const target = computed(() => internalPath(route.query.redirect) ?? { name: 'servers' })

async function submit() {
	if (busy.value) return

	if (next.value.length < MINIMUM_LENGTH) {
		failure.value = formatMessage(failures.weak_password)
		return
	}
	if (next.value !== confirmation.value) {
		failure.value = formatMessage(messages.mismatch)
		return
	}

	busy.value = true
	failure.value = null

	try {
		await api.auth.changePassword({ current_password: current.value, new_password: next.value })
		await refresh()
		await router.replace(target.value)
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		failure.value = formatMessage(failures[code] ?? messages.failed)
		current.value = ''
	} finally {
		busy.value = false
	}
}

async function leave() {
	await signOut()
	await router.push({ name: 'login' })
}
</script>
