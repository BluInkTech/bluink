// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdtemp, rm, symlink } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { platformPackage } from './package-launcher.mjs'

const launcher = fileURLToPath(
	new URL('./package-launcher.mjs', import.meta.url),
)

test('each released platform maps to its own addon package', () => {
	assert.equal(
		platformPackage({ platform: 'win32', arch: 'x64' }),
		'@bluink/cli-win32-x64',
	)
	assert.equal(
		platformPackage({ platform: 'darwin', arch: 'arm64' }),
		'@bluink/cli-darwin-arm64',
	)
	assert.equal(
		platformPackage({ platform: 'linux', arch: 'x64', glibc: '2.39' }),
		'@bluink/cli-linux-x64-gnu',
	)
})

test('platforms without a build get no package', () => {
	assert.equal(
		platformPackage({ platform: 'linux', arch: 'x64', glibc: null }),
		undefined,
	)
	assert.equal(platformPackage({ platform: 'darwin', arch: 'x64' }), undefined)
	assert.equal(platformPackage({ platform: 'win32', arch: 'arm64' }), undefined)
})

test('a missing platform package explains how to reinstall', async (t) => {
	// Run a copy outside any node_modules tree, so no platform package resolves.
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-launcher-'))
	t.after(() => rm(directory, { recursive: true, force: true }))
	const copy = path.join(directory, 'bluink.mjs')
	await copyFile(launcher, copy)
	// A global npm command on Unix invokes the launcher through a symlink.
	const command =
		process.platform === 'win32' ? copy : path.join(directory, 'bluink')
	if (command !== copy) await symlink(copy, command)
	const { BLUINK_ADDON: _unused, ...env } = process.env
	const result = spawnSync(process.execPath, [command, '--version'], {
		env,
		encoding: 'utf8',
	})
	assert.equal(result.status, 1)
	if (platformPackage())
		assert.match(
			result.stderr,
			/is not installed\. Reinstall Bluink without --omit=optional/,
		)
	else assert.match(result.stderr, /has no build for/)
})
