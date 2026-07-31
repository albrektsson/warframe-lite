# RPM spec for warframe-lite, using Fedora's Rust packaging macros.
#
# Fedora Rust builds are offline: dependencies come from a vendor tarball rather
# than from crates.io at build time. Produce the two sources with:
#
#   git archive --format=tar.gz --prefix=warframe-lite-%{version}/ \
#     -o ~/rpmbuild/SOURCES/warframe-lite-%{version}.tar.gz HEAD
#   cargo vendor vendor && \
#     tar caf ~/rpmbuild/SOURCES/warframe-lite-%{version}-vendor.tar.xz vendor
#
# then: rpmbuild -ba packaging/warframe-lite.spec
#
# The same spec can drive a COPR repo (upload the spec + both tarballs, or point
# COPR at the git repo with a make_srpm script that regenerates them).

%global crate warframe-lite
%global bin_name wf-lite

Name:           warframe-lite
Version:        0.0.1
Release:        1%{?dist}
Summary:        Linux-native, Overwolf-free Warframe companion overlay

License:        MIT
URL:            https://github.com/albrektsson/warframe-lite
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

# rust-packaging provides the %cargo_* macros; rust + cargo do the build.
BuildRequires:  rust-packaging

# libwayland-client is dlopen()'d at runtime, not linked, so it is a runtime
# (not build) dependency; tesseract backs the relic reward OCR.
Requires:       libwayland-client
Requires:       tesseract

ExclusiveArch:  x86_64

%description
warframe-lite is a lightweight, Linux-native alternative to AlecaFrame for
Warframe, targeting KDE Plasma (Wayland) with the game under Steam Proton. It
shows a click-through wlr-layer-shell overlay with live world state (Void
Fissures, the Void Trader, and the Cetus/Vallis/Cambion cycles) and an automatic
relic reward picker that ranks the on-screen rewards by warframe.market plat
price and flags primes you have already mastered. It observes the game only —
no Overwolf, no memory reading, no account credentials.

%prep
%autosetup -n %{name}-%{version} -a1
%cargo_prep -v vendor

%build
%cargo_build -- --bin %{bin_name}

%install
%cargo_install -- --bin %{bin_name}

%files
%license LICENSE
%doc README.md AGENT.md
%{_bindir}/%{bin_name}

%changelog
* Wed Jul 30 2026 Emil Albrektsson <61695840+albrektsson@users.noreply.github.com> - 0.0.1-1
- Initial package
