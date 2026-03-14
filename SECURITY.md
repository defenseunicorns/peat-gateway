# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in peat-gateway, please report it responsibly. **Do not open a public GitHub issue for security vulnerabilities.**

### How to Report

You have two options:

1. **Email**: Send a detailed report to [security@defenseunicorns.com](mailto:security@defenseunicorns.com)
2. **GitHub Security Advisories**: Use the [private vulnerability reporting](https://github.com/defenseunicorns/peat-gateway/security/advisories/new) feature on this repository

### What to Include

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

### Response Timeline

- **Acknowledgment**: Within 3 business days
- **Initial assessment**: Within 10 business days
- **Fix timeline**: Dependent on severity

### Disclosure Policy

- We will acknowledge reporters in the remediation PR (unless anonymity is requested)
- We follow coordinated disclosure practices
- We aim to release patches before public disclosure

## Supported Versions

| Version | Supported |
|---------|-----------|
| latest  | Yes       |

## Security-Relevant Areas

peat-gateway is the control plane gateway for the peat ecosystem. The following areas are particularly security-sensitive:

- **Envelope encryption**: Data encryption key wrapping, key hierarchy, and secret storage
- **OIDC federation**: Identity provider integration, token validation, and claim mapping
- **Enrollment tokens**: Token generation, scoping, expiration, and single-use enforcement
- **Multi-tenant isolation**: Tenant boundary enforcement, resource access control, and data segregation

When integrating peat-gateway, follow these practices:

- Rotate enrollment tokens and restrict their scope to the minimum necessary
- Validate OIDC tokens against the expected issuer and audience
- Ensure envelope encryption keys are stored in a secure backend
- Audit tenant isolation boundaries when modifying access control logic
- Keep dependencies up to date
