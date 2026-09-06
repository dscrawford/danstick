"""padmap: Switch-style controller assignment for RetroArch.

Assign player order by pressing a button, then republish the controllers as
virtual pads in that order so RetroArch binds them predictably.

The reason this exists rather than a config file: on multi-port adapters,
several ports can be indistinguishable by every attribute the kernel exposes.
See FINDINGS.md.
"""

__version__ = "0.1.0"
