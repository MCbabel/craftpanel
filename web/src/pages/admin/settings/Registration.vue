<template>
	<Card class="!mb-0 flex flex-col gap-4">
		<div class="flex items-center justify-between gap-4">
			<label class="flex flex-col gap-0.5" for="registration-enabled">
				<span class="text-lg font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</span>
				<span class="text-sm text-secondary">
					{{ formatMessage(messages.description) }}
				</span>
			</label>
			<Toggle id="registration-enabled" v-model="settings.registration_enabled" />
		</div>

		<Admonition
			v-if="settings.registration_enabled && !mailReady"
			type="warning"
			:title="formatMessage(messages.mailMissingTitle)"
		>
			{{ formatMessage(messages.mailMissingBody) }}
			<RouterLink class="text-link hover:underline" :to="{ name: 'admin-mail' }">
				{{ formatMessage(messages.mailMissingLink) }}
			</RouterLink>
		</Admonition>

		<div class="flex items-center justify-between gap-4">
			<label
				class="flex flex-col gap-0.5"
				:class="{ 'opacity-50': !settings.registration_enabled }"
				for="registration-approval"
			>
				<span class="font-semibold text-contrast">
					{{ formatMessage(messages.approvalTitle) }}
				</span>
				<span class="text-sm text-secondary">
					{{ formatMessage(messages.approvalDescription) }}
				</span>
			</label>
			<Toggle
				id="registration-approval"
				v-model="settings.registration_requires_approval"
				:disabled="!settings.registration_enabled"
			/>
		</div>

		<Admonition
			v-if="settings.registration_enabled && !settings.registration_requires_approval"
			type="warning"
			:body="formatMessage(messages.withoutApproval)"
		/>

		<p v-if="settings.registration_enabled" class="m-0 text-sm text-secondary">
			{{ formatMessage(messages.queueHint) }}
			<RouterLink class="text-link hover:underline" :to="{ name: 'admin-registrations' }">
				{{ formatMessage(messages.queueLink) }}
			</RouterLink>
		</p>
	</Card>
</template>

<script setup lang="ts">
import { Admonition, Card, defineMessages, Toggle, useVIntl } from '@modrinth/ui'
import { onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'

import type { PanelSettings } from '@/api'
import { mail } from '@/api/mail'

const { formatMessage } = useVIntl()

const settings = defineModel<PanelSettings>({ required: true })

const messages = defineMessages({
	title: { id: 'admin.settings.registration.title', defaultMessage: 'Sign-ups' },
	description: {
		id: 'admin.settings.registration.description',
		defaultMessage: 'Let people create their own account instead of you typing every one in.',
	},
	approvalTitle: {
		id: 'admin.settings.registration.approval-title',
		defaultMessage: 'I let every new account in myself',
	},
	approvalDescription: {
		id: 'admin.settings.registration.approval-description',
		defaultMessage:
			'A confirmed sign-up waits in a list until you say yes. Without this the account works the moment the address is confirmed.',
	},
	withoutApproval: {
		id: 'admin.settings.registration.without-approval',
		defaultMessage:
			'Anybody with an email address then gets an account and the default limits, without you seeing it first.',
	},
	mailMissingTitle: {
		id: 'admin.settings.registration.mail-missing-title',
		defaultMessage: 'No mail, no sign-ups',
	},
	mailMissingBody: {
		id: 'admin.settings.registration.mail-missing-body',
		defaultMessage:
			'A sign-up needs a confirmation mail, so the form stays closed until sending works — this switch alone does nothing.',
	},
	mailMissingLink: {
		id: 'admin.settings.registration.mail-missing-link',
		defaultMessage: 'Set up mail',
	},
	queueHint: {
		id: 'admin.settings.registration.queue-hint',
		defaultMessage: 'Sign-ups that are waiting are listed under',
	},
	queueLink: { id: 'admin.settings.registration.queue-link', defaultMessage: 'Sign-ups' },
})

const mailReady = ref(true)

onMounted(async () => {
	try {
		mailReady.value = (await mail.settings()).state !== 'not_configured'
	} catch {
	}
})
</script>
