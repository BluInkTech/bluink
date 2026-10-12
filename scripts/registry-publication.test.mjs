// SPDX-License-Identifier: MPL-2.0
import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtemp, rm, writeFile, access, readFile } from 'node:fs/promises'
import path from 'node:path'
import os from 'node:os'
import {
	installRegistryPackages,
	auditRegistrySignatures,
	provenanceStatement,
	provenancePolicy,
	publishRecords,
} from './registry-publication.mjs'
import { run } from './release-utils.mjs'

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

test('real npm subprocess diagnostics drive install and signature catalogue retries', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-npm-process-'))
	try {
		const cli = path.join(directory, 'npm-fixture.mjs')
		const calls = path.join(directory, 'calls.json')
		await writeFile(
			cli,
			`import { readFileSync, writeFileSync } from 'node:fs';
const args = process.argv.slice(2);
const calls = JSON.parse(readFileSync(process.env.BLUINK_TEST_NPM_CALLS, 'utf8'));
calls.push(args);
writeFileSync(process.env.BLUINK_TEST_NPM_CALLS, JSON.stringify(calls));
if (calls.filter(call => call[0] === args[0]).length === 1) {
 console.error('npm error code ETARGET');
 console.error('npm error notarget No matching version found for @bluink/cli@26.10.7-alpha.');
 process.exit(1);
}
if (args[0] === 'audit') console.log(JSON.stringify({ invalid: [], missing: [], verified: [] }));
`,
		)
		await writeFile(calls, '[]')
		const moduleUrl = new URL('./registry-publication.mjs', import.meta.url)
			.href
		await run(
			process.execPath,
			[
				'--input-type=module',
				'-e',
				`import { installRegistryPackages, auditRegistrySignatures } from ${JSON.stringify(moduleUrl)};
const directory = process.env.BLUINK_TEST_NPM_DIRECTORY;
const records = [{ name: '@bluink/cli', version: '26.10.7-alpha' }];
const options = { pause: async () => {}, attempts: 2 };
await installRegistryPackages(directory, records, options);
await auditRegistrySignatures(directory, records, options);`,
			],
			{
				env: {
					...process.env,
					npm_execpath: cli,
					BLUINK_TEST_NPM_CALLS: calls,
					BLUINK_TEST_NPM_DIRECTORY: directory,
				},
				capture: true,
			},
		)
		const actual = JSON.parse(await readFile(calls, 'utf8'))
		assert.deepEqual(
			actual.map((args) => args[0]),
			['install', 'install', 'audit', 'audit'],
		)
		for (const args of actual) {
			assert.ok(args.includes('--prefer-online'))
			assert.equal(
				args[args.indexOf('--cache') + 1],
				path.join(directory, 'cache'),
			)
		}
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
})

test('signature verification does not retry invalid signatures or unrelated registry errors', async () => {
	for (const error of [
		new Error('npm error code E401'),
		new Error('npm error code EINTEGRITY'),
		new Error('npm error code EATTESTATIONVERIFY'),
		new Error(
			'npm error code ETARGET\nNo matching version found for unrelated@1.0.0.',
		),
	]) {
		let audits = 0
		await assert.rejects(
			auditRegistrySignatures(os.tmpdir(), records, {
				audit: async () => {
					audits++
					throw error
				},
				pause: async () => {},
				attempts: 2,
			}),
			(actual) => actual === error,
		)
		assert.equal(audits, 1)
	}
})

test('installation waits for catalogue propagation without retrying unrelated failures', async () => {
	const directory = await mkdtemp(path.join(os.tmpdir(), 'bluink-catalogue-'))
	try {
		const delayed = new Error(
			`npm error code ETARGET\nnpm error notarget No matching version found for ${records[2].name}@${records[2].version}.`,
		)
		let installs = 0
		let pauses = 0
		await writeFile(path.join(directory, 'package-lock.json'), '{}')
		await installRegistryPackages(directory, records, {
			install: async () => {
				if (++installs < 3) throw delayed
			},
			pause: async () => {
				pauses++
				await assert.rejects(access(path.join(directory, 'package-lock.json')))
			},
			attempts: 3,
		})
		assert.equal(installs, 3)
		assert.equal(pauses, 2)
		for (const error of [
			delayed,
			new Error('npm error code E401'),
			new Error('npm error code EINTEGRITY'),
			new Error(
				'npm error code ETARGET\nNo matching version found for unrelated@1.0.0.',
			),
		]) {
			installs = 0
			await assert.rejects(
				installRegistryPackages(directory, records, {
					install: async () => {
						installs++
						throw error
					},
					pause: async () => {},
					attempts: 2,
				}),
				(actual) => actual === error,
			)
			assert.equal(installs, error === delayed ? 2 : 1)
		}
	} finally {
		await rm(directory, { recursive: true, force: true })
	}
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

test('first publication can remain processing beyond the old thirty-second window', async () => {
	let lookups = 0
	let uploads = 0
	await publishRecords([records[0]], {
		lookup: async () => (++lookups < 21 ? undefined : registry(records[0])),
		upload: async () => {
			uploads++
		},
		pause: async () => {},
	})
	assert.equal(lookups, 21)
	assert.equal(uploads, 1)
})

function reportFor(record) {
	const policy = provenancePolicy(record)
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
						path: policy.workflow,
						ref: policy.ref,
					},
				},
				resolvedDependencies: [
					{
						uri: `git+${identity.publicRepository}@${policy.ref}`,
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

test('resources bind their own workflow and tag, and their source commit is independent of CLI', () => {
	for (const record of records.slice(0, 2)) {
		const { statement, report } = reportFor(record)
		statement.predicate.buildDefinition.resolvedDependencies[0].digest.gitCommit =
			'b'.repeat(40)
		const update = () => {
			report.verified[0].attestationBundles[0].bundle.dsseEnvelope.payload =
				Buffer.from(JSON.stringify(statement)).toString('base64')
		}
		update()
		assert.equal(
			provenanceStatement(report, record, identity).commit,
			'b'.repeat(40),
		)
		assert.throws(
			() =>
				provenanceStatement(report, record, {
					...identity,
					packageName: record.name,
				}),
			/wrong workflow/,
		)
		statement.predicate.buildDefinition.externalParameters.workflow.path =
			'.github/workflows/public-release.yml'
		update()
		assert.throws(
			() => provenanceStatement(report, record, identity),
			/wrong workflow/,
		)
		statement.predicate.buildDefinition.externalParameters.workflow.path =
			provenancePolicy(record).workflow
		statement.predicate.buildDefinition.externalParameters.workflow.ref = `refs/tags/v${record.version}`
		update()
		assert.throws(
			() => provenanceStatement(report, record, identity),
			/wrong workflow/,
		)
	}
	assert.throws(
		() => provenancePolicy({ name: '@bluink/unknown', version: '26.10.1' }),
		/Unknown/,
	)
})
