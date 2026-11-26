// The edgeboot v4 check's job token: run 3, job 9 on the bed's shard, signed by seed [7; 32], whose public half the
// bed's RUN_SERVICE_PUBLIC is, with the `scp` claim `runsvc::sign_job_token` writes. A test key: never an install's.
const c = require('crypto');
const key = c.createPrivateKey({
  key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), Buffer.alloc(32, 7)]),
  format: 'der',
  type: 'pkcs8',
});
const b = (o) => Buffer.from(JSON.stringify(o)).toString('base64url');
const guid = (kind, id) => {
  const h = id.toString(16).padStart(15, '0');
  return `00000000-0000-4${kind.toString(16).padStart(3, '0')}-8${h.slice(0, 3)}-${h.slice(3)}`;
};
const scp = `Actions.Results:${guid(2, 3)}:${guid(1, 9)}`;
const signed = `${b({ typ: 'JWT', alg: 'EdDSA' })}.${b({ chain: 'actions:7100001.11', k: 'job', job: 9, attempt: 1, exp: 4102444800, scp })}`;
console.log(`${signed}.${c.sign(null, Buffer.from(signed), key).toString('base64url')}`);
