<template>
	<div class="mx-auto flex min-h-full w-full max-w-[27rem] flex-col justify-center px-6 py-12">
		<Card class="!mb-0 flex flex-col gap-6">
			<div class="flex flex-col gap-1">
				<h1 class="m-0 text-2xl font-extrabold text-contrast">
					{{ formatMessage(messages.title) }}
				</h1>
			</div>

			<p class="m-0 text-secondary">{{ formatMessage(messages.body) }}</p>
			<p class="m-0 text-secondary">{{ formatMessage(messages.mail) }}</p>

			<ButtonLink :to="{ name: 'login' }" size="lg" class="!w-full !justify-center">
				{{ formatMessage(commonMessages.signInButton) }}
			</ButtonLink>
		</Card>
	</div>
</template>

<script setup lang="ts">
import { ButtonLink, Card, commonMessages, defineMessages, useVIntl } from '@modrinth/ui'

const { formatMessage } = useVIntl()

const messages = defineMessages({
	title: {
		id: 'auth.registration-pending.title',
		defaultMessage: 'Waiting to be let in',
	},
	body: {
		id: 'auth.registration-pending.body',
		defaultMessage:
			'Your address is confirmed. On this panel an administrator looks at every new account before it works.',
	},
	mail: {
		id: 'auth.registration-pending.mail',
		defaultMessage: 'You will get a mail as soon as that has happened. There is nothing to do here.',
	},
})
</script>
