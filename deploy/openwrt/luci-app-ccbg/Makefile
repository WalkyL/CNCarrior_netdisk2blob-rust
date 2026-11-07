# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
#
# LuCI app package for the CCBG Gateway admin panel (embedded iframe).
# This package is architecture-independent (pure JS/JSON) so a single .ipk
# covers both x86_64 and arm OpenWRT targets (PKGARCH:=all). The gatewayd
# daemon itself is packaged separately per architecture.

include $(TOPDIR)/rules.mk

LUCI_TITLE:=LuCI app for CCBG Gateway admin panel
LUCI_DEPENDS:=+luci-base
LUCI_PKGARCH:=all

PKG_NAME:=luci-app-ccbg
PKG_VERSION:=1.0
PKG_RELEASE:=1

PKG_LICENSE:=LicenseRef-CCBG-Commercial
PKG_MAINTAINER:=walky <walky@agi2030.cc>

include ../../luci.mk

# call BuildPackage - OpenWrt buildroot signature
