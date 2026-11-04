# Fail the build when the kernel config merge silently dropped a fragment option.
define OHMLATL_CHECK_KCONFIG
	$(BR2_EXTERNAL_OHMLATL_PATH)/board/ohmlatl/check-kconfig.sh \
		$(BR2_EXTERNAL_OHMLATL_PATH)/board/ohmlatl/linux.fragment $(LINUX_DIR)/.config
endef
LINUX_POST_BUILD_HOOKS += OHMLATL_CHECK_KCONFIG
