pub(super) fn native_line_ending() -> &'static str {
    #[cfg(windows)]
    {
        "\r\n"
    }

    #[cfg(not(windows))]
    {
        "\n"
    }
}
