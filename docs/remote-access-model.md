# Remote Access Model

This note records the assumptions behind Den's current remote access design.

## Deployment Model

- A set of Den servers is usually operated by a single person.
- The same person may access multiple Den instances from browsers, tablets, and other Den instances.
- Closed networks, VPNs, and host/VM topologies are first-class deployment targets.
- Public Internet exposure and public CA-backed deployment are not the primary baseline.

## Authentication Assumptions

- Each Den should have its own password.
- Reusing the same password across multiple Den instances is discouraged.
- Remote access features should avoid storing passwords unless explicitly designed otherwise.
- Long-lived cross-Den trust relationships are not the default assumption.

## Transport Assumptions

- Browser-to-Den and Den-to-Den access should converge on the same transport model where practical.
- HTTPS/WSS is preferred for both browser access and future Quick Connect / relay flows.
- In closed networks, self-signed certificates are an acceptable bootstrap mechanism.
- Fingerprint confirmation is therefore an important part of the trust model.

## Threat Model

Den currently optimizes for:

- preventing passive eavesdropping on local networks, VPNs, and host/VM links
- reducing accidental credential reuse across multiple Den instances
- keeping ad hoc remote access simple for single-user operation

Den does not assume:

- a multi-user trust domain with fine-grained peer-to-peer delegation as the primary case
- publicly trusted certificates for every deployment
- automatic trust establishment without user confirmation

## Design Consequences

- The old trusted-peer model is being phased out in favor of Quick Connect.
- Self-signed TLS is acceptable, but certificate fingerprints must be surfaced clearly.
- Remote access flows should be explicit and user-directed.
- Relay, when added, should be explicit and constrained rather than automatic.

## Documentation Rule

User-facing changes to browser access, Den-to-Den access, Quick Connect, relay, TLS, or fingerprint handling should update:

- `README.md`
- `README.ja.md`
- this document when the underlying assumptions or security model change
