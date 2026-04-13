#pragma once

#include <SFML/Graphics.hpp>

#include <optional>
#include <string>
#include <vector>

namespace ui {

struct MenuItem {
	std::string id;
	std::string label;
	std::string value;
	bool adjustable = false;
};

struct MenuAction {
	std::string itemId;
	int adjustDelta = 0;
	bool activate = false;
};

class MenuOverlay {
   public:
	void setActive(bool active);
	[[nodiscard]] bool active() const { return active_; }

	void setItems(const std::vector<MenuItem>& items);
	[[nodiscard]] std::optional<MenuAction> handleKeyPress(sf::Keyboard::Key key) const;
	[[nodiscard]] std::optional<MenuAction> handleMouseClick(const sf::Vector2i& pixel,
	                                                         const sf::Vector2u& windowSize);
	void moveSelection(int delta);

	[[nodiscard]] int selectedIndex() const { return selectedIndex_; }
	void draw(sf::RenderWindow& window, const sf::Font& font) const;

   private:
	[[nodiscard]] sf::FloatRect itemRect(std::size_t idx, const sf::Vector2u& winSize) const;

	bool active_ = false;
	int selectedIndex_ = 0;
	std::vector<MenuItem> items_;
};

}  // namespace ui
