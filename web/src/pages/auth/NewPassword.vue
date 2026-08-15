<template>
	<div class="mx-auto flex min-h-full w-full max-w-[27rem] flex-col justify-center px-6 py-12">
		<Card class="!mb-0 flex flex-col gap-6">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
				<p v-if="username !== null" class="m-0 text-secondary">
					{{ formatMessage(messages.forWhom, { username }) }}
				</p>
			</div>

			<div v-if="state === 'checking'" class="flex items-center gap-2 text-secondary">
				<SpinnerIcon class="animate-spin" />
				{{ formatMessage(messages.checking) }}
			</div>

			<template v-else-if="state !== 'ready'">
				<Admonition
					type="critical"
					:body="
						formatMessage(state === 'unreachable' ? messages.unreachable : messages.dead)
					"
				/>
				<ButtonLink
					:to="{ name: 'forgot-password' }"
					type="colored"
					color="brand"
					size="lg"
					class="!w-full !justify-center"
				>
					{{ formatMessage(messages.askAgain) }}
				</ButtonLink>
			</template>

			<form v-else class="flex flex-col gap-4" @submit.prevent="submit">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="new-password">
						{{ formatMessage(messages.newPassword) }}
					</label>
					<StyledInput
						id="new-password"
						v-model="chosen"
						type="password"
						name="new-password"
						autocomplete="new-password"
						:disabled="busy"
						:input-attrs="{ autofocus: true }"
						wrapper-class="w-full"
					/>
					<span class="text-sm text-secondary">
						{{ formatMessage(messages.lengthHint, { length: PASSWORD_MIN_LENGTH }) }}
					</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="confirm-password">
						{{ formatMessage(commonMessages.confirmPasswordLabel) }}
					</label>
					<StyledInput
						id="confirm-password"
						v-model="repeated"
						type="password"
						name="confirm-password"
						autocomplete="new-password"
						:disabled="busy"
						wrapper-class="w-full"
					/>
				</div>

				<Admonition v-if="failure" type="critical" :body="failure" />

				<Button
					native-type="submit"
					type="colored"
					color="brand"
					size="lg"
					:disabled="!ready"
					:loading="busy"
					class="!w-full !justify-center"
				>
					<SpinnerIcon v-if="busy" class="animate-spin" />
					{{ formatMessage(messages.submit) }}
				</Button>

				<p class="m-0 text-center text-sm text-secondary">
					{{ formatMessage(messages.signInAfterwards) }}
				</p>
			</form>
		</Card>
	</div>
</template>

<script setup lang="ts">
import { SpinnerIcon } from '@modrinth/assets'
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
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import { isApiRequestError } from '@/api'
import { recovery } from '@/api/recovery'
import { PASSWORD_MIN_LENGTH } from '@/pages/auth/register'
import {
	confirmProblem,
	type LinkState,
	linkStateOfError,
	newPasswordReady,
	tokenFromLocation,
} from '@/pages/auth/recovery'

const { formatMessage } = useVIntl()
const router = useRouter()

const messages = defineMessages({
	title: { id: 'auth.new-password.title', defaultMessage: 'Set a new password' },
	forWhom: { id: 'auth.new-password.for-whom', defaultMessage: 'For {username}.' },
	checking: { id: 'auth.new-password.checking', defaultMessage: 'Checking the link…' },
	dead: {
		id: 'auth.new-password.dead',
		defaultMessage:
			'This link is no longer valid: it has been used, or it has run out. Ask for a new one.',
	},
	unreachable: {
		id: 'auth.new-password.unreachable',
		defaultMessage: 'The panel could not be reached. Try the link again in a moment.',
	},
	askAgain: { id: 'auth.new-password.ask-again', defaultMessage: 'Ask for a new link' },
	newPassword: { id: 'auth.new-password.new-password', defaultMessage: 'New password' },
	lengthHint: { id: 'auth.new-password.length-hint', defaultMessage: 'At least {length} characters.' },
	submit: { id: 'auth.new-password.submit', defaultMessage: 'Set password' },
	signInAfterwards: {
		id: 'auth.new-password.sign-in-afterwards',
		defaultMessage: 'Afterwards you sign in with the new password. Every open session is closed.',
	},
	failed: { id: 'auth.new-password.failed', defaultMessage: 'That did not work. Please try again.' },
})

const failures: Record<string, MessageDescriptor> = defineMessages({
	weak_password: {
		id: 'auth.new-password.error.weak-password',
		defaultMessage: 'That password is too short.',
	},
	invalid_reset_token: {
		id: 'auth.new-password.error.invalid-token',
		defaultMessage: 'This link is no longer valid. Ask for a new one.',
	},
	too_many_attempts: {
		id: 'auth.new-password.error.too-many-attempts',
		defaultMessage: 'Too many attempts. Wait a few minutes before trying again.',
	},
})

const state = ref<LinkState>('checking')
const username = ref<string | null>(null)
const chosen = ref('')
const repeated = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)
let token: string | null = null

const ready = computed(() => newPasswordReady(chosen.value, repeated.value, PASSWORD_MIN_LENGTH))

onMounted(async () => {
	token = tokenFromLocation({ search: window.location.search, hash: window.location.hash })

	if (token !== null) {
		window.history.replaceState(window.history.state, '', window.location.pathname)
	}

	if (token === null) {
		state.value = 'dead'
		return
	}

	try {
		username.value = (await recovery.whose(token)).username
		state.value = 'ready'
	} catch (error) {
		state.value = linkStateOfError(isApiRequestError(error) ? error.code : '')
	}
})

async function submit() {
	if (busy.value || !ready.value || token === null) return
	busy.value = true
	failure.value = null

	try {
		await recovery.confirm(token, chosen.value)
		await router.replace({ name: 'login', query: { reset: 'done' } })
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		const problem = confirmProblem(code)
		failure.value = formatMessage(failures[problem] ?? messages.failed)
		if (problem === 'invalid_reset_token') state.value = 'dead'
	} finally {
		busy.value = false
	}
}
</script>
