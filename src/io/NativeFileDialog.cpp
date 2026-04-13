#include "io/NativeFileDialog.hpp"

#include <array>
#include <cstdio>
#include <memory>
#include <sstream>
#include <string>

namespace io {

namespace {

std::string shellEscapeSingleQuoted(const std::string& value) {
	std::string out;
	out.reserve(value.size() + 8);
	out.push_back('\'');
	for (char c : value) {
		if (c == '\'') {
			out += "'\"'\"'";
		} else {
			out.push_back(c);
		}
	}
	out.push_back('\'');
	return out;
}

std::optional<std::string> runAndCaptureLine(const std::string& command) {
	std::array<char, 1024> buffer{};
	std::string output;
	const std::unique_ptr<FILE, decltype(&pclose)> pipe(popen(command.c_str(), "r"), pclose);
	if (!pipe) {
		return std::nullopt;
	}
	while (fgets(buffer.data(), static_cast<int>(buffer.size()), pipe.get()) != nullptr) {
		output += buffer.data();
	}
	if (output.empty()) {
		return std::nullopt;
	}
	while (!output.empty() && (output.back() == '\n' || output.back() == '\r')) {
		output.pop_back();
	}
	if (output.empty()) {
		return std::nullopt;
	}
	return output;
}

}  // namespace

std::optional<std::string> NativeFileDialog::pickOpenPath(const std::string& title,
                                                          const std::string& extensionWithoutDot) {
#if defined(__APPLE__)
	std::ostringstream cmd;
	cmd << "osascript -e "
	    << shellEscapeSingleQuoted("set pickedFile to choose file with prompt \"" + title + "\"")
	    << " -e " << shellEscapeSingleQuoted("if pickedFile is missing value then return \"\"")
	    << " -e " << shellEscapeSingleQuoted("set p to POSIX path of pickedFile") << " -e "
	    << shellEscapeSingleQuoted("if p ends with \"." + extensionWithoutDot + "\" then return p")
	    << " -e " << shellEscapeSingleQuoted("return p");
	return runAndCaptureLine(cmd.str());
#else
	(void)title;
	(void)extensionWithoutDot;
	return std::nullopt;
#endif
}

std::optional<std::string> NativeFileDialog::pickSavePath(const std::string& title,
                                                          const std::string& suggestedName,
                                                          const std::string& extensionWithoutDot) {
#if defined(__APPLE__)
	std::ostringstream cmd;
	cmd << "osascript -e "
	    << shellEscapeSingleQuoted("set pickedFile to choose file name with prompt \"" + title +
	                               "\" default name \"" + suggestedName + "\"")
	    << " -e " << shellEscapeSingleQuoted("if pickedFile is missing value then return \"\"")
	    << " -e " << shellEscapeSingleQuoted("set p to POSIX path of pickedFile") << " -e "
	    << shellEscapeSingleQuoted("if p does not end with \"." + extensionWithoutDot +
	                               "\" then set p to p & \"." + extensionWithoutDot + "\"")
	    << " -e " << shellEscapeSingleQuoted("return p");
	return runAndCaptureLine(cmd.str());
#else
	(void)title;
	(void)suggestedName;
	(void)extensionWithoutDot;
	return std::nullopt;
#endif
}

}  // namespace io
