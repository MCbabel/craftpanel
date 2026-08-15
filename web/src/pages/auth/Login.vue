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
				v-if="justReset"
				type="success"
				:body="formatMessage(messages.passwordSet)"
			/>

			<form class="flex flex-col gap-4" @submit.prevent="submit">
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="username">
						{{ formatMessage(commonMessages.usernameLabel) }}
					</label>
					<StyledInput
						id="username"
						v-model="username"
						name="username"
						autocomplete="username"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:disabled="busy"
						:input-attrs="{ autofocus: true }"
						wrapper-class="w-full"
					/>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="password">
						{{ formatMessage(commonMessages.passwordLabel) }}
					</label>
					<StyledInput
						id="password"
						v-model="password"
						type="password"
						name="password"
						autocomplete="current-password"
						:disabled="busy"
						wrapper-class="w-full"
					/>
					<RouterLink
						v-if="options?.password_reset_enabled"
						class="self-start text-sm text-link"
						:to="{ name: 'forgot-password' }"
					>
						{{ formatMessage(messages.forgot) }}
					</RouterLink>
				</div>

				<template v-if="blocked === 'email_unverified'">
					<Admonition type="warning" :body="formatMessage(messages.emailUnverified)" />
					<div v-if="!resent" class="flex flex-col gap-1.5">
						<label class="text-sm font-semibold text-contrast" for="resend-email">
							{{ formatMessage(messages.resendEmailLabel) }}
						</label>
						<StyledInput
							id="resend-email"
							v-model="resendEmail"
							type="email"
							name="resend-email"
							autocomplete="email"
							autocapitalize="none"
							autocorrect="off"
							:spellcheck="false"
							:disabled="resending"
							:input-attrs="{ inputmode: 'email' }"
							wrapper-class="w-full"
						/>
						<Button
							size="lg"
							class="!w-full !justify-center"
							:loading="resending"
							:disabled="resendEmail.trim() === ''"
							@click="resend"
						>
							<SpinnerIcon v-if="resending" class="animate-spin" />
							{{ formatMessage(messages.resend) }}
						</Button>
					</div>
					<Admonition v-else type="info" :body="formatMessage(messages.mailSent)" />
				</template>
				<Admonition v-else-if="failure" type="critical" :body="failure" />

				<Button
					native-type="submit"
					type="colored"
					color="brand"
					size="lg"
					:disabled="!username || !password"
					:loading="busy"
					class="!w-full !justify-center"
				>
					<SpinnerIcon v-if="busy" class="animate-spin" />
					{{ formatMessage(commonMessages.signInButton) }}
				</Button>

				<p v-if="options?.registration_enabled" class="m-0 text-center text-sm text-secondary">
					{{ formatMessage(messages.noAccount) }}
					<RouterLink class="text-link" :to="{ name: 'register' }">
						{{ formatMessage(messages.createOne) }}
					</RouterLink>
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
	Card,
	commonMessages,
	defineMessages,
	type MessageDescriptor,
	StyledInput,
	useVIntl,
} from '@modrinth/ui'
import { computed, onMounted, ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'

import { api, isApiRequestError } from '@/api'
import { type AuthOptions, registrations } from '@/api/registration'
import { useSession } from '@/composables/session'
import { type SignInBlock, signInBlock } from '@/pages/auth/register'
import { internalPath } from '@/router'

const { formatMessage } = useVIntl()
const route = useRoute()
const router = useRouter()
const { adopt } = useSession()

const messages = defineMessages({
	title: {
		id: 'auth.login.title',
		defaultMessage: 'Sign in',
	},
	subtitle: {
		id: 'auth.login.subtitle',
		defaultMessage: 'Sign in with your panel account.',
	},
	failed: {
		id: 'auth.login.failed',
		defaultMessage: 'Sign-in failed. Please try again.',
	},
	forgot: {
		id: 'auth.login.forgot',
		defaultMessage: 'Forgot your password?',
	},
	noAccount: {
		id: 'auth.login.no-account',
		defaultMessage: 'No account yet?',
	},
	createOne: {
		id: 'auth.login.create-one',
		defaultMessage: 'Create one',
	},
	passwordSet: {
		id: 'auth.login.password-set',
		defaultMessage: 'Your password is set. Sign in with the new one.',
	},
	emailUnverified: {
		id: 'auth.login.email-unverified',
		defaultMessage:
			'Check your inbox: your address is not confirmed yet, so the account cannot be used.',
	},
	resend: {
		id: 'auth.login.resend',
		defaultMessage: 'Send the confirmation mail again',
	},
	resendEmailLabel: {
		id: 'auth.login.resend-email',
		defaultMessage: 'The address you signed up with',
	},
	mailSent: {
		id: 'auth.login.mail-sent',
		defaultMessage:
			'If that address has a sign-up waiting, a new link is on its way. Check your inbox.',
	},
})

const failures: Record<string, MessageDescriptor> = defineMessages({
	invalid_credentials: {
		id: 'auth.login.error.invalid-credentials',
		defaultMessage: 'Incorrect username or password.',
	},
	too_many_attempts: {
		id: 'auth.login.error.too-many-attempts',
		defaultMessage: 'Too many attempts. Wait a few minutes before trying again.',
	},
	rate_limited: {
		id: 'auth.login.error.rate-limited',
		defaultMessage: 'Too many requests. Wait a moment before trying again.',
	},
	network_unreachable: {
		id: 'auth.login.error.network-unreachable',
		defaultMessage: 'The panel could not be reached.',
	},
})

const username = ref('')
const password = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)
const blocked = ref<SignInBlock>(null)
const options = ref<AuthOptions | null>(null)
const resendEmail = ref('')
const resending = ref(false)
const resent = ref(false)

const justReset = computed(() => route.query.reset === 'done')

onMounted(async () => {
	try {
		options.value = await registrations.options()
	} catch {
	}
})

async function submit() {
	if (busy.value) return
	busy.value = true
	failure.value = null
	blocked.value = null

	try {
		adopt(await api.auth.login({ username: username.value, password: password.value }))
		await router.replace(internalPath(route.query.redirect) ?? { name: 'servers' })
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		const block = signInBlock(code)
		password.value = ''

		if (block === 'approval_pending') {
			await router.replace({ name: 'registration-pending' })
			return
		}

		blocked.value = block
		failure.value = block === null ? formatMessage(failures[code] ?? messages.failed) : null
	} finally {
		busy.value = false
	}
}

async function resend() {
	if (resending.value || resendEmail.value.trim() === '') return
	resending.value = true
	try {
		await registrations.resendVerification(resendEmail.value.trim())
	} catch {
	} finally {
		resent.value = true
		resending.value = false
	}
}
</script>
