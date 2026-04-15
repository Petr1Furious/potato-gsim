#include "ui/MenuOverlay.hpp"

#include <algorithm>

namespace ui {

void MenuOverlay::setActive(bool active) {
	active_ = active;
	if (!active_) {
		selectedIndex_ = 0;
	} else if (!items_.empty()) {
		selectedIndex_ = std::clamp(selectedIndex_, 0, static_cast<int>(items_.size() - 1));
	}
}

void MenuOverlay::setItems(const std::vector<MenuItem>& items) {
	items_ = items;
	if (!items_.empty()) {
		selectedIndex_ = std::clamp(selectedIndex_, 0, static_cast<int>(items_.size() - 1));
	} else {
		selectedIndex_ = 0;
	}
}

void MenuOverlay::moveSelection(int delta) {
	if (items_.empty()) {
		return;
	}
	selectedIndex_ = std::clamp(selectedIndex_ + delta, 0, static_cast<int>(items_.size() - 1));
}

std::optional<MenuAction> MenuOverlay::handleKeyPress(sf::Keyboard::Key key) const {
	if (!active_ || items_.empty()) {
		return std::nullopt;
	}
	const MenuItem& item = items_[selectedIndex_];
	if (key == sf::Keyboard::Key::Enter) {
		return MenuAction{.itemId = item.id, .adjustDelta = 0, .activate = true};
	}
	if (item.adjustable && key == sf::Keyboard::Key::Left) {
		return MenuAction{.itemId = item.id, .adjustDelta = -1, .activate = false};
	}
	if (item.adjustable && key == sf::Keyboard::Key::Right) {
		return MenuAction{.itemId = item.id, .adjustDelta = 1, .activate = false};
	}
	return std::nullopt;
}

std::optional<MenuAction> MenuOverlay::handleMouseClick(const sf::Vector2i& pixel,
                                                        const sf::Vector2u& windowSize) {
	if (!active_) {
		return std::nullopt;
	}
	for (std::size_t i = 0; i < items_.size(); ++i) {
		if (itemRect(i, windowSize)
		        .contains(sf::Vector2f(static_cast<float>(pixel.x), static_cast<float>(pixel.y)))) {
			selectedIndex_ = static_cast<int>(i);
			return MenuAction{
			    .itemId = items_[i].id,
			    .adjustDelta = 0,
			    .activate = true,
			};
		}
	}
	return std::nullopt;
}

sf::FloatRect MenuOverlay::itemRect(std::size_t idx, const sf::Vector2u& winSize) const {
	const float panelWidth = 560.0f;
	const float rowHeight = 28.0f;
	const float headerHeight = 50.0f;
	const float panelHeight = headerHeight + 14.0f + static_cast<float>(items_.size()) * rowHeight;
	const float desiredTop = std::max(20.0f, static_cast<float>(winSize.y) * 0.08f);
	const float maxTop = std::max(10.0f, static_cast<float>(winSize.y) - panelHeight - 10.0f);
	const float panelTop = std::clamp(desiredTop, 10.0f, maxTop);
	const float panelLeft = (static_cast<float>(winSize.x) - panelWidth) * 0.5f;
	return sf::FloatRect(sf::Vector2f(panelLeft + 14.0f, panelTop + headerHeight + rowHeight * idx),
	                     sf::Vector2f(panelWidth - 28.0f, rowHeight - 2.0f));
}

void MenuOverlay::draw(sf::RenderWindow& window, const sf::Font& font) const {
	if (!active_) {
		return;
	}

	const sf::View oldView = window.getView();
	window.setView(window.getDefaultView());

	const sf::Vector2u size = window.getSize();
	sf::RectangleShape dim(sf::Vector2f(static_cast<float>(size.x), static_cast<float>(size.y)));
	dim.setFillColor(sf::Color(0, 0, 0, 150));
	window.draw(dim);

	const float panelWidth = 560.0f;
	const float rowHeight = 28.0f;
	const float headerHeight = 50.0f;
	const float panelHeight = headerHeight + 14.0f + static_cast<float>(items_.size()) * rowHeight;
	const float desiredTop = std::max(20.0f, static_cast<float>(size.y) * 0.08f);
	const float maxTop = std::max(10.0f, static_cast<float>(size.y) - panelHeight - 10.0f);
	const float panelTop = std::clamp(desiredTop, 10.0f, maxTop);
	const float panelLeft = (static_cast<float>(size.x) - panelWidth) * 0.5f;

	sf::RectangleShape panel(sf::Vector2f(panelWidth, panelHeight));
	panel.setPosition(sf::Vector2f(panelLeft, panelTop));
	panel.setFillColor(sf::Color(18, 22, 30, 230));
	panel.setOutlineColor(sf::Color(110, 140, 190, 240));
	panel.setOutlineThickness(1.0f);
	window.draw(panel);

	sf::Text header(font, "Settings (Esc to close)", 20);
	header.setPosition(sf::Vector2f(panelLeft + 14.0f, panelTop + 12.0f));
	header.setFillColor(sf::Color(240, 245, 255));
	window.draw(header);

	for (std::size_t i = 0; i < items_.size(); ++i) {
		const sf::FloatRect rowRect = itemRect(i, size);
		sf::RectangleShape rowBg(rowRect.size);
		rowBg.setPosition(rowRect.position);
		const bool selected = static_cast<int>(i) == selectedIndex_;
		rowBg.setFillColor(selected ? sf::Color(70, 95, 135, 180) : sf::Color(30, 35, 45, 120));
		window.draw(rowBg);

		const MenuItem& item = items_[i];
		sf::Text label(font, item.label, 14);
		label.setPosition(rowRect.position + sf::Vector2f(8.0f, 3.0f));
		label.setFillColor(sf::Color(230, 235, 250));
		window.draw(label);

		sf::Text value(font, item.value, 14);
		sf::FloatRect bounds = value.getLocalBounds();
		value.setPosition(sf::Vector2f(rowRect.position.x + rowRect.size.x - bounds.size.x - 12.0f,
		                               rowRect.position.y + 3.0f));
		value.setFillColor(item.adjustable ? sf::Color(200, 230, 250) : sf::Color(200, 210, 230));
		window.draw(value);
	}

	window.setView(oldView);
}

}  // namespace ui
