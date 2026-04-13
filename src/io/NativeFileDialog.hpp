#pragma once

#include <optional>
#include <string>

namespace io {

class NativeFileDialog {
   public:
	static std::optional<std::string> pickOpenPath(const std::string& title,
	                                               const std::string& extensionWithoutDot);
	static std::optional<std::string> pickSavePath(const std::string& title,
	                                               const std::string& suggestedName,
	                                               const std::string& extensionWithoutDot);
};

}  // namespace io
