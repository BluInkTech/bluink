// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import {
	smokeDependencies,
	validateResourceContext,
	validateResourceInput,
	validateResourceReports,
} from './resource-release.mjs'
import { targets } from './release-utils.mjs'

const input = {
	format: 'bluink.resource-input',
	formatVersion: 1,
	kind: 'fonts',
	version: '26.10.2',
	publicRepository: 'https://github.com/BluInkTech/bluink',
	artifact: {
		name: '@bluink/fonts',
		version: '26.10.2',
		filename: 'bluink-fonts-26.10.2.tgz',
		integrity: 'sha512-Ynl0ZXM=',
		sha256: 'b'.repeat(64),
	},
}
const context = {
	commit: 'a'.repeat(40),
	repository: 'BluInkTech/bluink',
	ref: 'refs/tags/fonts/v26.10.2',
	event: 'push',
	publicRepository: 'true',
	publishing: true,
	enabled: 'true',
	oidc: true,
}

test('independent publication binds one frozen package to its own protected tag and OIDC', () => {
	assert.equal(validateResourceInput(input), input)
	assert.equal(
		validateResourceContext(input, context).packageName,
		'@bluink/fonts',
	)
	for (const override of [
		{ repository: 'other/repo' },
		{ ref: 'refs/tags/v26.10.2' },
		{ ref: 'refs/tags/typst/v26.10.2' },
		{ ref: 'refs/heads/main' },
		{ event: 'workflow_dispatch' },
		{ publicRepository: 'false' },
		{ enabled: 'false' },
		{ oidc: false },
		{ commit: 'not-a-commit' },
	])
		assert.throws(() =>
			validateResourceContext(input, { ...context, ...override }),
		)
	assert.equal(
		validateResourceContext(input, {
			...context,
			publishing: false,
			event: 'workflow_dispatch',
			ref: 'refs/heads/main',
			enabled: 'false',
			oidc: false,
		}).publicCommit,
		context.commit,
	)
	for (const override of [
		{ privateRevision: 'a'.repeat(40) },
		{ kind: 'web' },
		{ version: 'latest' },
		{ artifact: { ...input.artifact, filename: '../outside.tgz' } },
		{ artifact: { ...input.artifact, archive: '/runner/private.tgz' } },
		{ artifact: { ...input.artifact, name: '@bluink/typst' } },
		{ artifact: { ...input.artifact, sha256: 'invalid' } },
	])
		assert.throws(() => validateResourceInput({ ...input, ...override }))
})

test('resource publication requires all platform reports for exactly the tested archive and source', () => {
	const identity = validateResourceContext(input, context)
	const reports = Object.fromEntries(
		targets.map((target) => [
			`${target.platform}-${target.arch}`,
			{
				status: 'passed',
				kind: input.kind,
				version: input.version,
				publicCommit: identity.publicCommit,
				platform: target.platform,
				arch: target.arch,
				archiveSha256: input.artifact.sha256,
			},
		]),
	)
	assert.equal(validateResourceReports(input, identity, reports).length, 3)
	for (const override of [
		{ status: 'failed' },
		{ kind: 'typst' },
		{ version: '26.10.3' },
		{ publicCommit: 'c'.repeat(40) },
		{ platform: 'win32' },
		{ arch: 'arm64' },
		{ archiveSha256: 'd'.repeat(64) },
	])
		assert.throws(
			() =>
				validateResourceReports(input, identity, {
					...reports,
					'linux-x64': { ...reports['linux-x64'], ...override },
				}),
			/Missing matching/,
		)
	assert.throws(
		() => validateResourceReports(input, identity, {}),
		/Missing matching/,
	)
})

test('installed Fonts smoke test installs its pinned shaping dependency', () => {
	const manifest = {
		devDependencies: {
			'@bluink/typst': '26.10.2',
			harfbuzzjs: '1.6.3',
		},
	}
	assert.deepEqual(smokeDependencies(input, manifest, '/fonts.tgz'), {
		'@bluink/fonts': 'file:/fonts.tgz',
		'@bluink/typst': '26.10.2',
		harfbuzzjs: '1.6.3',
	})
	assert.throws(
		() =>
			smokeDependencies(
				input,
				{
					devDependencies: {
						...manifest.devDependencies,
						harfbuzzjs: '^1.6.3',
					},
				},
				'/fonts.tgz',
			),
		/exact HarfBuzz/,
	)
})
