// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import test from 'node:test'
import { promisify } from 'node:util'
import { withTempDir } from '../test-utils/fixtures.js'

const execute = promisify(execFile)

test('compiled addon bridge still forwards host events when optional capture cannot be opened', async () => {
	await withTempDir(async (directory) => {
		const addon = new URL('../../dist/src/native/addon.js', import.meta.url)
			.href
		const logger = new URL('../../dist/src/logger.js', import.meta.url).href
		const { stdout, stderr } = await execute(
			process.execPath,
			[
				'--input-type=module',
				'-e',
				`
import { nativeAddon, shutdownNativeDiagnostics } from ${JSON.stringify(addon)};
import logger from ${JSON.stringify(logger)};
nativeAddon();
logger.diagnostic({ timestampMs: 1, level: 'warn', target: 'bluink.host', name: 'host_bridge_check' });
await shutdownNativeDiagnostics();
process.stdout.write('ok');
`,
			],
			{
				timeout: 10_000,
				env: {
					...process.env,
					BLUINK_DIAGNOSTICS_FILE: directory,
					BLUINK_LOG_LEVEL: 'info',
					NO_COLOR: '1',
				},
			},
		)
		assert.equal(stdout, 'ok')
		assert.match(stderr, /Optional diagnostics file unavailable/)
		assert.match(stderr, /WARN \[bluink.host\] host_bridge_check/)
		assert.doesNotMatch(stderr, /Native diagnostics are unavailable/)
		assert.equal(stderr.includes(directory), false)
	})
})
