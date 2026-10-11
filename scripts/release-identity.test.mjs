// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import { publicIdentity } from './release-identity.mjs'

test('distributed metadata accepts only public identity and enforces official release pairing', () => {
	const metadata = {
		publicCommit: 'a'.repeat(40),
		publicRepository: 'https://github.com/example/cli',
	}
	assert.equal(publicIdentity('26.10.7-alpha', metadata, true).channel, 'alpha')
	assert.equal(publicIdentity('26.10.8', metadata, true).channel, 'stable')
	for (const key of ['gitCommit', 'privateRevision', 'builtAt', 'arbitrary'])
		assert.throws(
			() => publicIdentity('26.10.7-alpha', { ...metadata, [key]: 'hidden' }),
			/Unexpected/,
		)
	assert.throws(
		() => publicIdentity('26.10.7-alpha', { ...metadata, channel: 'stable' }),
		/channel/,
	)
	assert.throws(
		() => publicIdentity('26.10.7-alpha', {}, true),
		/public source identity/,
	)
	assert.throws(
		() => publicIdentity('0.0.0-dev', metadata, true),
		/calendar version/,
	)
	assert.throws(
		() =>
			publicIdentity('26.10.7-alpha', {
				...metadata,
				publicRepository: 'https://user:secret@github.com/example/cli',
			}),
		/repository URL/,
	)
})
