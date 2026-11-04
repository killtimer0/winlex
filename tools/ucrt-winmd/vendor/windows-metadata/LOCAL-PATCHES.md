# Local architecture-merge changes

Based on windows-metadata 0.100.0, retaining its upstream license.

Changes to reading and merging metadata:

- Architecture neutrality requires all supported bits (X86|X64|Arm64), not merely
  all supplied inputs. A single tagged input therefore remains architecture-only.
- Class containers receive the union of their input architecture masks.
- Architecture member and type shape keys include flags, custom attributes and
  parameter metadata. Same signature blobs alone cannot establish equivalence:
  declaration-only calling conventions and SAL directions may differ.
- The reader exposes `FieldLayout` offsets, the merger copies them, and type shape
  keys distinguish offset-only variants.
- Signature decoding retains enclosing TypeRef scopes as qualified nested names,
  allowing rewriters to preserve references through a merge.

These changes support the generator's declaration-only CRT inputs; the merger is
not a general lossless rewriter for arbitrary ECMA-335 assemblies.
