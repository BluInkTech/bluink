// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import { provenanceStatement, publishRecords } from './registry-publication.mjs'

const identity = {
	publicRepository: 'https://github.com/example/cli',
	publicCommit: 'a'.repeat(40),
}
const records = ['fonts', 'typst', 'cli'].map((kind) => ({
	name: `@bluink/${kind}`,
	version: '26.10.7-alpha',
	integrity: 'sha512-Ynl0ZXM=',
	archive: `${kind}.tgz`,
}))
const registry = (record, integrity = record.integrity) => ({
	name: record.name,
	version: record.version,
	dist: {
		integrity,
		attestations: { url: 'https://registry.npmjs.org/attestations' },
	},
})

test('all versions and existing provenance are checked before any upload', async () => {
	let uploads = 0
	const upload = async () => {
		uploads++
	}
	await assert.rejects(
		publishRecords(records, {
			lookup: async (name) =>
				name === records[2].name
					? registry(records[2], 'different')
					: undefined,
			upload,
		}),
		/different bytes/,
	)
	assert.equal(uploads, 0)
	await assert.rejects(
		publishRecords(records, {
			lookup: async (name) =>
				name === records[0].name ? registry(records[0]) : undefined,
			upload,
			verifyExisting: async () => {
				throw new Error('Invalid signature')
			},
		}),
		/Invalid signature/,
	)
	assert.equal(uploads, 0)
})

test('lost upload responses recover the same published bytes and retries never upload twice', async () => {
	const published = new Map()
	let uploads = 0
	const options = {
		lookup: async (name) => published.get(name),
		upload: async (record) => {
			uploads++
			published.set(record.name, registry(record))
			throw new Error('Lost response')
		},
		pause: async () => {},
		attempts: 2,
	}
	await publishRecords(records, options)
	await publishRecords(records, options)
	assert.equal(uploads, 3)
	await assert.rejects(
		publishRecords(records, {
			...options,
			lookup: async (name) => ({
				...published.get(name),
				dist: { integrity: records[0].integrity },
			}),
		}),
		/verification pending/,
	)
	assert.equal(uploads, 3)
})

test('registry delay is bounded and integrity conflicts stop recovery', async () => {
	let lookups = 0
	await publishRecords([records[0]], {
		lookup: async () => (++lookups < 3 ? undefined : registry(records[0])),
		upload: async () => {},
		pause: async () => {},
		attempts: 3,
	})
	assert.equal(lookups, 3)
	await assert.rejects(
		publishRecords([records[0]], {
			lookup: async () => undefined,
			upload: async () => {},
			pause: async () => {},
			attempts: 2,
		}),
		/verification pending/,
	)
})

function reportFor(record) {
	const statement = {
		predicateType: 'https://slsa.dev/provenance/v1',
		subject: [
			{
				name: `pkg:npm/${record.name.replace('@', '%40')}@${record.version}`,
				digest: { sha512: Buffer.from('bytes').toString('hex') },
			},
		],
		predicate: {
			buildDefinition: {
				externalParameters: {
					workflow: {
						repository: identity.publicRepository,
						path: '.github/workflows/public-release.yml',
						ref: `refs/tags/v${record.version}`,
					},
				},
				resolvedDependencies: [
					{
						uri: `git+${identity.publicRepository}@refs/tags/v${record.version}`,
						digest: { gitCommit: identity.publicCommit },
					},
				],
			},
		},
	}
	return {
		statement,
		report: {
			invalid: [],
			missing: [],
			verified: [
				{
					name: record.name,
					version: record.version,
					attestationBundles: [
						{
							predicateType: statement.predicateType,
							bundle: {
								dsseEnvelope: {
									payload: Buffer.from(JSON.stringify(statement)).toString(
										'base64',
									),
								},
							},
						},
					],
				},
			],
		},
	}
}

test('verified npm provenance must bind the exact package, bytes, public workflow and CLI source', () => {
	const record = records[2]
	const { report } = reportFor(record)
	assert.equal(
		provenanceStatement(report, record, identity).commit,
		identity.publicCommit,
	)
	for (const mutate of [
		(statement) => {
			statement.subject[0].digest.sha512 = 'wrong'
		},
		(statement) => {
			statement.subject[0].name = 'pkg:npm/other@1.0.0'
		},
		(statement) => {
			statement.predicate.buildDefinition.externalParameters.workflow.path =
				'.github/workflows/other.yml'
		},
		(statement) => {
			statement.predicate.buildDefinition.externalParameters.workflow.repository =
				'https://github.com/other/cli'
		},
		(statement) => {
			statement.predicate.buildDefinition.resolvedDependencies[0].digest.gitCommit =
				'b'.repeat(40)
		},
	]) {
		const fixture = reportFor(record)
		mutate(fixture.statement)
		fixture.report.verified[0].attestationBundles[0].bundle.dsseEnvelope.payload =
			Buffer.from(JSON.stringify(fixture.statement)).toString('base64')
		assert.throws(
			() => provenanceStatement(fixture.report, record, identity),
			/wrong workflow/,
		)
	}
	assert.throws(
		() => provenanceStatement({ ...report, verified: [] }, record, identity),
		/Missing verified/,
	)
	assert.throws(
		() => provenanceStatement({ ...report, invalid: [{}] }, record, identity),
		/did not pass/,
	)
})
